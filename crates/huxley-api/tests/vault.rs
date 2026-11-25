//! The `vault` module's API against a real PostgreSQL (design doc §8.5):
//! who may see, reveal and manage client secrets, and the reveal trail.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "vault api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    owner: String,
}

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

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(&pools).await;
    // The module is instance-wide: switch it on (other tests may have too).
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "vault"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Vault MSP").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("vault").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Desk User", PASSWORD)
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

impl Setup {
    async fn call(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", cookie);
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn ok(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(cookie, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri}: {status} {value}");
        value
    }
}

struct People {
    tech: String,
    reader: String,
    outsider: String,
}

async fn people(s: &Setup) -> People {
    let client = org(
        &s.pools,
        s.provider,
        huxley_orgs::OrgKind::Client,
        "Vault client",
    )
    .await;
    People {
        tech: login(
            &s.app,
            &user(&s.pools, Some(("technician", s.provider))).await.1,
        )
        .await,
        reader: login(
            &s.app,
            &user(&s.pools, Some(("read_only", s.provider))).await.1,
        )
        .await,
        outsider: login(
            &s.app,
            &user(&s.pools, Some(("technician", client))).await.1,
        )
        .await,
    }
}

/// Reveals a secret, checking the response isn't cached: the secret.
async fn reveal_uncached(s: &Setup, cookie: &str, id: &str, reason: &str) -> String {
    let req = Request::post(format!("/api/v1/vault/items/{id}/reveal"))
        .header("x-requested-with", "huxley")
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "reason": reason }).to_string()))
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["cache-control"], "no-store, private");
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<Value>(&bytes).unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn technicians_reveal_readers_only_see_and_others_nothing() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let body = json!({ "orgId": s.provider, "name": "Firewall admin", "username": "admin", "url": "https://10.0.0.1", "secret": "Hunter2!" });
    let item = s
        .ok(&p.tech, "POST", "/api/v1/vault/items", Some(body))
        .await;
    let id = item["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            item["hasSecret"].as_bool(),
            item["canReveal"].as_bool(),
            item["canManage"].as_bool()
        ),
        (Some(true), Some(true), Some(true))
    );
    assert!(!item.to_string().contains("Hunter2"));

    // Readers see that it exists, not the secret; they can't change it.
    let listed = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/vault/items?orgId={}", s.provider),
            None,
        )
        .await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == id.as_str())
    );
    assert!(!listed.to_string().contains("Hunter2"));
    let (status, _) = s
        .call(
            &p.reader,
            "POST",
            &format!("/api/v1/vault/items/{id}/reveal"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .call(
            &p.reader,
            "PATCH",
            &format!("/api/v1/vault/items/{id}"),
            Some(json!({ "version": item["version"], "name": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let seen = s
        .ok(&p.reader, "GET", &format!("/api/v1/vault/items/{id}"), None)
        .await;
    assert_eq!(
        (seen["canReveal"].as_bool(), seen["canManage"].as_bool()),
        (Some(false), Some(false))
    );

    // A technician reveals it, uncached, and the reveal is on record.
    assert_eq!(
        reveal_uncached(&s, &p.tech, &id, "Ticket T-7").await,
        "Hunter2!"
    );
    let log = s
        .ok(
            &p.reader,
            "GET",
            &format!("/api/v1/vault/items/{id}/reveals"),
            None,
        )
        .await;
    assert_eq!(
        (log[0]["reason"].as_str(), log.as_array().unwrap().len()),
        (Some("Ticket T-7"), 1)
    );
}

#[tokio::test]
async fn other_clients_see_nothing_and_the_trail_outlives_the_item() {
    let Some(s) = setup().await else { return };
    let p = people(&s).await;
    let body = json!({ "orgId": s.provider, "name": "Firewall admin", "secret": "Hunter2!" });
    let item = s
        .ok(&p.tech, "POST", "/api/v1/vault/items", Some(body))
        .await;
    let id = item["id"].as_str().unwrap().to_owned();
    reveal_uncached(&s, &p.tech, &id, "Audit").await;
    // Another client's technician can't see or reveal it.
    let (status, _) = s
        .call(
            &p.outsider,
            "GET",
            &format!("/api/v1/vault/items/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = s
        .call(
            &p.outsider,
            "POST",
            &format!("/api/v1/vault/items/{id}/reveal"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let theirs = s.ok(&p.outsider, "GET", "/api/v1/vault/items", None).await;
    assert!(
        theirs
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != id.as_str())
    );

    // Deleted; the trail stays for those who may read the vault.
    let (status, _) = s
        .call(
            &p.tech,
            "DELETE",
            &format!("/api/v1/vault/items/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let all = s.ok(&s.owner, "GET", "/api/v1/vault/reveals", None).await;
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .any(|r| r["itemName"] == "Firewall admin" && r["itemId"].is_null())
    );
}
