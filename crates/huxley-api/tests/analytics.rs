//! The semantic data layer's API (ADR-041) against a real PostgreSQL:
//! numbers cover only the organizations where you can read the source.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "analytics api passphrase";

async fn org(pools: &TestPools, parent: Uuid, kind: huxley_orgs::OrgKind, name: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = huxley_orgs::create(&mut tx, &ctx, parent, kind, &unique(name))
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    id
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("analytics").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Analytics User", PASSWORD)
        .await
        .unwrap();
    match role_at {
        None => huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap(),
        Some((role, org)) => {
            sqlx::query(
                "INSERT INTO role_assignments (id, user_id, role_id, org_id) \
                 SELECT $1, $2, id, $3 FROM roles WHERE key = $4 AND is_system",
            )
            .bind(Uuid::now_v7())
            .bind(u.id)
            .bind(org)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit().await.unwrap();
    (u.id, email)
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

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-requested-with", "huxley")
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn numbers_cover_only_what_you_can_read() {
    let Some(db) = isolated("analytics_api").await else {
        return;
    };
    let pools = db.pools.clone();
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let acme = org(&pools, root, huxley_orgs::OrgKind::Client, "Acme").await;
    let globex = org(&pools, root, huxley_orgs::OrgKind::Client, "Globex").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    for (org, n) in [(acme, 2), (globex, 3)] {
        for i in 0..n {
            let (status, t) = call(
                &app,
                &owner,
                "POST",
                "/api/v1/tickets",
                json!({ "orgId": org, "title": format!("Ticket {i}") }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{t}");
        }
    }
    let by_client = json!({
        "source": "tickets", "measures": [{ "fn": "count" }], "dimensions": [{ "field": "client" }],
        "filters": [{ "field": "open", "op": "eq", "value": true }],
    });

    // The owner sees both clients, biggest first.
    let (status, answer) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/analytics/query",
        by_client.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let rows = answer["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{answer}");
    assert_eq!(rows[0][1], 3);

    // A read-only user at Acme sees Acme's numbers only.
    let reader = login(&app, &user(&pools, Some(("read_only", acme))).await.1).await;
    let (_, catalog) = call(
        &app,
        &reader,
        "GET",
        "/api/v1/analytics/sources",
        Value::Null,
    )
    .await;
    let keys: Vec<&str> = catalog["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"tickets"), "{catalog}");
    assert!(
        catalog["ranges"]
            .as_array()
            .unwrap()
            .contains(&json!("last_30_days"))
    );
    let (status, answer) = call(&app, &reader, "POST", "/api/v1/analytics/query", by_client).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["rows"],
        json!([[answer["rows"][0][0], 2]]),
        "{answer}"
    );

    // Bad specs point at what's wrong.
    let (status, problem) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/analytics/query",
        json!({ "source": "tickets", "measures": [{ "fn": "sum", "field": "priority" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        problem["errors"][0]["pointer"], "/measures/0/field",
        "{problem}"
    );
}
