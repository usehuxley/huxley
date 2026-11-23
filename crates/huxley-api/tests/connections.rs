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

const OAUTH_SPEC: &str = r#"
openapi: 3.0.3
info: { title: Signed API, version: "1" }
servers: [{ url: "https://signed.example.com" }]
components:
  securitySchemes:
    user:
      type: oauth2
      flows:
        authorizationCode:
          authorizationUrl: https://login.example.com/authorize
          tokenUrl: https://login.example.com/token
          scopes: { read: Read }
paths:
  /me:
    get: { responses: { "200": { description: ok } } }
"#;

#[tokio::test]
async fn sign_in_redirects_to_the_provider_and_back_to_the_app() {
    let Some(pools) = pools().await else { return };
    // Without a public URL, providers couldn't send anyone back.
    let plain = app(&pools);
    let app = app_with_public_url(&pools);
    let msp = provider_org(&pools).await;
    let email = user(&pools, true).await.1;
    let owner = login(&app, &email).await;
    let (_, integration) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": msp, "spec": OAUTH_SPEC })),
    )
    .await;
    let (status, connection) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(json!({
            "orgId": msp,
            "integrationId": integration["id"],
            "connection": { "name": "Me", "authScheme": "user",
                            "settings": { "client_id": "huxley-app" },
                            "secrets": { "client_secret": "s" } }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{connection}");
    assert_eq!(connection["status"], "needs_auth");
    let start = format!(
        "/api/v1/connections/{}/sign-in",
        connection["id"].as_str().unwrap()
    );

    let plain_owner = login(&plain, &email).await;
    let (status, _) = call(&plain, "POST", &start, &plain_owner, None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, started) = call(&app, "POST", &start, &owner, None).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let url = started["authorizeUrl"].as_str().unwrap();
    assert!(
        url.starts_with("https://login.example.com/authorize?"),
        "{url}"
    );
    assert!(url.contains("client_id=huxley-app"));
    assert!(
        url.contains(
            "redirect_uri=https%3A%2F%2Fhuxley.example.com%2Fintegrations%2Foauth%2Fcallback"
        ),
        "{url}"
    );
    assert!(url.contains("code_challenge_method=S256"));

    // A made-up state is refused before any provider is called.
    let (status, problem) = call(
        &app,
        "POST",
        "/api/v1/connections/sign-in/complete",
        &owner,
        Some(json!({ "state": "made-up", "code": "c" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "state");
}

fn app_with_public_url(pools: &TestPools) -> axum::Router {
    let config = AuthConfig {
        public_url: Some("https://huxley.example.com/".into()),
        ..AuthConfig::default()
    };
    let auth = AuthService::new(pools.app.clone(), config, crypto(pools));
    router(AppState::new(
        pools.app.clone(),
        auth,
        crypto(pools),
        ApiConfig {
            login_attempts_per_minute: 1000,
            ..ApiConfig::default()
        },
    ))
}

#[tokio::test]
async fn only_sign_in_connections_start_a_sign_in() {
    let Some(pools) = pools().await else { return };
    let app = app_with_public_url(&pools);
    let msp = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    // Connections that don't sign in can't start one.
    let (_, basic) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": msp, "spec": SPEC })),
    )
    .await;
    let (_, other) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(json!({
            "orgId": msp,
            "integrationId": basic["id"],
            "connection": { "name": "Basic", "authScheme": "basic",
                            "settings": { "username": "u" }, "secrets": { "password": "p" } }
        })),
    )
    .await;
    let (status, _) = call(
        &app,
        "POST",
        &format!(
            "/api/v1/connections/{}/sign-in",
            other["id"].as_str().unwrap()
        ),
        &owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn built_in_integrations_are_added_from_the_catalog() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let msp = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (status, catalog) = call(&app, "GET", "/api/v1/integrations/catalog", &owner, None).await;
    assert_eq!(status, StatusCode::OK);
    let graph = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "msgraph")
        .unwrap();
    assert_eq!(graph["actions"], 14);

    let add = |org: Uuid| json!({ "orgId": org });
    let (status, added) = call(
        &app,
        "POST",
        "/api/v1/integrations/catalog/msgraph",
        &owner,
        Some(add(msp)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(added["key"], "msgraph");
    let id = added["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/integrations/catalog/msgraph",
        &owner,
        Some(add(msp)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "added once per organization");
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/integrations/catalog/nope",
        &owner,
        Some(add(msp)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Its token URL takes the tenant, which must be safe.
    let connection = |tenant: &str| {
        json!({ "orgId": msp, "integrationId": id, "connection": {
            "name": "Contoso", "authScheme": "app", "serverVariables": { "tenant": tenant },
            "settings": { "client_id": "abc" }, "secrets": { "client_secret": "s3cret" } } })
    };
    let (status, problem) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(connection("evil.com/x")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "serverVariables.tenant");
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/connections",
        &owner,
        Some(connection("contoso.onmicrosoft.com")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Technicians can't add integrations.
    let tech = login(&app, &user(&pools, false).await.1).await;
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/integrations/catalog/slack",
        &tech,
        Some(add(msp)),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}
