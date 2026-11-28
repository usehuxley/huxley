//! Malware scanning of uploads end to end (ADR-094): an upload waits for its
//! scan, a clean one is then served, and an infected one is quarantined,
//! audited, listed for administrators, scanned again or deleted. The
//! scanner is a fake clamd speaking INSTREAM (`huxley-scan`'s tests also
//! run against a real one).

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_scan::{ClamavConfig, Provider, ScanConfig, Scanner};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt as _;

const PASSWORD: &str = "file scan passphrase";

/// The EICAR test file, put together at runtime so this source isn't flagged.
fn eicar() -> Vec<u8> {
    [
        r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
}

/// A clamd that finds EICAR and passes everything else.
async fn fake_clamd() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut command = Vec::new();
                while let Ok(b) = socket.read_u8().await {
                    if b == 0 {
                        break;
                    }
                    command.push(b);
                }
                let mut data = Vec::new();
                if command == b"zINSTREAM" {
                    loop {
                        let len = socket.read_u32().await.unwrap() as usize;
                        if len == 0 {
                            break;
                        }
                        let mut chunk = vec![0; len];
                        socket.read_exact(&mut chunk).await.unwrap();
                        data.extend(chunk);
                    }
                }
                let bad = eicar();
                let reply: &[u8] = if command == b"zVERSION" {
                    b"ClamAV 1.4.7\0"
                } else if data.windows(bad.len()).any(|w| w == bad) {
                    b"stream: Eicar-Signature FOUND\0"
                } else {
                    b"stream: OK\0"
                };
                let _ = socket.write_all(reply).await;
            });
        }
    });
    address
}

struct Setup {
    app: axum::Router,
    engine: huxley_engine::Engine,
}

async fn setup(pools: &TestPools) -> Setup {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let scanner = Scanner::from_config(&ScanConfig {
        provider: Provider::Clamav,
        clamav: ClamavConfig {
            address: fake_clamd().await,
            timeout_seconds: 5,
        },
        ..ScanConfig::default()
    })
    .unwrap()
    .unwrap();
    let storage = huxley_storage::Storage::memory().with_scanning(true, true);
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto.clone())
        .with_scanner(scanner.clone(), storage.clone());
    let state = AppState {
        scanner: Some(Arc::new(scanner)),
        ..AppState::new(pools.app.clone(), auth, crypto, config).with_storage(storage)
    };
    Setup {
        app: router(state),
        engine,
    }
}

async fn owner(pools: &TestPools) -> String {
    let email = format!("{}@example.com", unique("scan").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Scan Owner", PASSWORD)
        .await
        .unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    email
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(app: &axum::Router, method: &str, uri: &str, cookie: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-requested-with", "huxley")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    send(app, req).await
}

async fn login(app: &axum::Router, email: &str) -> String {
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "email": email, "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

async fn upload(app: &axum::Router, uri: &str, cookie: &str, name: &str, data: &[u8]) -> Value {
    let boundary = "huxleyboundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::post(uri)
        .header("x-requested-with", "huxley")
        .header("cookie", cookie)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let (status, file) = send(app, req).await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    file
}

async fn download(app: &axum::Router, id: &str, cookie: &str) -> (StatusCode, Value) {
    call(
        app,
        "GET",
        &format!("/api/v1/attachments/{id}/content"),
        cookie,
    )
    .await
}

/// The item in a JSON list whose `field` is `value`.
fn by(list: &Value, field: &str, value: &str) -> Option<Value> {
    list.as_array()?.iter().find(|f| f[field] == value).cloned()
}

async fn counted(pools: &TestPools, sql: &'static str, id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(&pools.owner)
        .await
        .unwrap()
}

/// Scans until nothing of this test's is left pending.
async fn scan(engine: &huxley_engine::Engine) {
    while engine.scan_uploads().await.unwrap() > 0 {}
}

#[tokio::test]
async fn uploads_wait_for_a_clean_scan_and_infected_ones_are_quarantined() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let Setup { app, engine } = setup(&pools).await;
    let cookie = login(&app, &owner(&pools).await).await;
    let uri = format!("/api/v1/entities/organization/{root}/attachments");

    // A clean file waits for its scan, then downloads.
    let file = upload(&app, &uri, &cookie, "report.txt", b"quarterly numbers").await;
    assert_eq!(file["scanStatus"], "pending");
    let id = file["id"].as_str().unwrap().to_owned();
    let (status, problem) = download(&app, &id, &cookie).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["type"].as_str().unwrap().ends_with("file-scanning"),
        "{problem}"
    );
    scan(&engine).await;
    assert_eq!(download(&app, &id, &cookie).await.0, StatusCode::OK);

    // An infected one is quarantined: never served, audited, listed.
    let bad = upload(&app, &uri, &cookie, "invoice.pdf.exe", &eicar()).await;
    let bad_id = bad["id"].as_str().unwrap().to_owned();
    scan(&engine).await;
    let (status, problem) = download(&app, &bad_id, &cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        problem["type"]
            .as_str()
            .unwrap()
            .ends_with("file-quarantined"),
        "{problem}"
    );
    let (_, files) = call(
        &app,
        "GET",
        &format!("/api/v1/entities/organization/{root}/attachments"),
        &cookie,
    )
    .await;
    let listed = by(&files, "id", &bad_id).unwrap();
    assert_eq!(listed["scanStatus"], "infected");

    let (status, quarantined) = call(&app, "GET", "/api/v1/platform/file-scans", &cookie).await;
    assert_eq!(status, StatusCode::OK, "{quarantined}");
    let entry = by(&quarantined, "recordId", &bad_id).expect("the quarantined file is listed");
    assert_eq!(entry["finding"], "Eicar-Signature");
    assert_eq!(entry["engine"], "clamav");
    let bad_uuid: uuid::Uuid = bad_id.parse().unwrap();
    let audits =
        "SELECT count(*) FROM audit_events WHERE action = 'file.quarantined' AND target_id = $1";
    assert_eq!(counted(&pools, audits, bad_uuid).await, 1);
    let events = "SELECT count(*) FROM events_outbox WHERE event_type = 'file.quarantined' AND subject_id = $1";
    assert_eq!(counted(&pools, events, bad_uuid).await, 1);

    let (status, scanning) = call(&app, "GET", "/api/v1/platform/file-scans/status", &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scanning["provider"], "clamav");
    assert_eq!(scanning["reachable"], true);
    assert_eq!(scanning["holdsUnscanned"], true);
    assert!(scanning["infected"].as_i64().unwrap() >= 1);

    // Scanned again, it's quarantined again; deleted, the attachment goes.
    let scan_id = entry["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/platform/file-scans/{scan_id}/rescan"),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        download(&app, &bad_id, &cookie).await.0,
        StatusCode::CONFLICT
    );
    scan(&engine).await;
    assert_eq!(
        download(&app, &bad_id, &cookie).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/platform/file-scans/{scan_id}"),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        download(&app, &bad_id, &cookie).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, after) = call(&app, "GET", "/api/v1/platform/file-scans", &cookie).await;
    assert!(
        after
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["recordId"] != bad_id.as_str())
    );
}
