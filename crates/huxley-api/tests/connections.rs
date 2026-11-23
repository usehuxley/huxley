//! Connection endpoints over HTTP, against a real PostgreSQL.

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

const PASSWORD: &str = "connection api passphrase";

const SPEC: &str = r#"
openapi: 3.0.3
info: { title: Billing, version: "1" }
servers: [{ url: "https://billing.example.com" }]
components:
  securitySchemes:
    basic: { type: http, scheme: basic }
paths:
  /invoices:
    get: { responses: { "200": { description: ok } } }
"#;

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

fn app(pools: &TestPools) -> axum::Router {
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(
        pools.app.clone(),
        auth,
        crypto(pools),
        config,
    ))
}

async fn user(pools: &TestPools, owner: bool) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("int").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Integration User", PASSWORD)
        .await
        .unwrap();
    if owner {
        huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    (u.id, email)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
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
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

async fn provider_org(pools: &TestPools) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Int"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    org.id
}

async fn child_org(pools: &TestPools, parent: Uuid, kind: huxley_orgs::OrgKind) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(&mut tx, &ctx, parent, kind, &unique("Child"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    org.id
}

#[tokio::test]
async fn connections_keep_secrets_write_only() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let msp = provider_org(&pools).await;
    let client = child_org(&pools, msp, huxley_orgs::OrgKind::Client).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (_, integration) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": msp, "spec": SPEC })),
    )
    .await;
    let integration_id = integration["id"].as_str().unwrap().to_owned();

    // A client org uses its MSP's integration with its own credentials.
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(json!({
            "orgId": client,
            "integrationId": integration_id,
            "connection": {
                "name": "Client billing",
                "authScheme": "basic",
                "settings": { "username": "client-svc" },
                "secrets": { "password": "s3cret-pass" }
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["secretFields"], json!(["password"]));
    assert_eq!(created["settings"], json!({ "username": "client-svc" }));
    assert!(!created.to_string().contains("s3cret-pass"));
    let id = created["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/v1/connections/{id}");

    // Changing the name without secrets keeps the password.
    let (status, updated) = call(
        &app,
        "PUT",
        &uri,
        &owner,
        Some(json!({
            "name": "Billing",
            "authScheme": "basic",
            "settings": { "username": "client-svc" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["secretFields"], json!(["password"]));

    let (_, listed) = call(
        &app,
        "GET",
        &format!("/api/v1/connections?integrationId={integration_id}"),
        &owner,
        None,
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // Technicians see connections but can't change them.
    let (tech, email) = user(&pools, false).await;
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id) \
         SELECT $1, $2, id, $3 FROM roles WHERE key = 'technician'",
    )
    .bind(Uuid::now_v7())
    .bind(tech)
    .bind(msp)
    .execute(&pools.owner)
    .await
    .unwrap();
    let tech = login(&app, &email).await;
    assert_eq!(call(&app, "GET", &uri, &tech, None).await.0, StatusCode::OK);
    assert_eq!(
        call(&app, "DELETE", &uri, &tech, None).await.0,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        call(&app, "DELETE", &uri, &owner, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", &uri, &owner, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn connections_are_checked_before_saving() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let msp = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (_, integration) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": msp, "spec": SPEC })),
    )
    .await;
    let integration_id = integration["id"].as_str().unwrap().to_owned();

    // A missing secret is a 422 on that field.
    let (status, problem) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(json!({
            "orgId": msp,
            "integrationId": integration_id,
            "connection": { "name": "No password", "authScheme": "basic", "settings": { "username": "u" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "secrets.password");

    // An org outside the integration's tree can't use it.
    let other = provider_org(&pools).await;
    let (status, problem) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(json!({
            "orgId": other,
            "integrationId": integration_id,
            "connection": { "name": "Elsewhere", "authScheme": "basic",
                            "settings": { "username": "u" }, "secrets": { "password": "p" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "integrationId");
}
