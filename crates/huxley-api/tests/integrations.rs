//! Integration endpoints over HTTP, against a real PostgreSQL.

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

const PASSWORD: &str = "integration api passphrase";

const SPEC_V1: &str = r#"
openapi: 3.0.3
info: { title: Widget Service, version: "1.0" }
servers: [{ url: "https://widgets.example.com" }]
components:
  securitySchemes:
    key: { type: apiKey, in: header, name: X-Key }
security: [{ key: [] }]
paths:
  /widgets:
    get:
      operationId: listWidgets
      responses:
        "200":
          description: ok
          content: { application/json: { schema: { type: array, items: { type: string } } } }
  /widgets/{id}:
    get:
      operationId: listWidgets
      responses: { "200": { description: ok } }
"#;

const SPEC_V2: &str = r#"{
  "openapi": "3.1.0",
  "info": { "title": "Widget Service", "version": "2.0" },
  "paths": { "/widgets": { "post": { "operationId": "makeWidget", "responses": { "201": { "description": "made" } } } } }
}"#;

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

#[tokio::test]
async fn specs_import_as_versioned_integrations() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let org = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;

    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": org, "spec": SPEC_V1 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["key"], "widget-service");
    assert_eq!(created["currentVersion"], 1);
    let actions = created["definition"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["auth"], json!(["key"]));
    let warnings: Vec<&str> = created["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        warnings,
        vec![
            "operation id \"listWidgets\" is used twice; this one is \"listWidgets_2\"",
            "path parameter {id} isn't declared; added as a string",
        ]
    );

    // The same key again in this org is a conflict; a bad spec is a 422.
    let again = json!({ "orgId": org, "spec": SPEC_V1 });
    let (status, _) = call(&app, "POST", "/api/v1/integrations", &owner, Some(again)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let bad = json!({ "orgId": org, "key": "other", "spec": "title: not a spec" });
    let (status, problem) = call(&app, "POST", "/api/v1/integrations", &owner, Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "spec");

    // A newer spec becomes version 2.
    let (status, updated) = call(
        &app,
        "PUT",
        &format!("/api/v1/integrations/{id}/spec"),
        &owner,
        Some(json!({ "spec": SPEC_V2 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["currentVersion"], 2);
    assert_eq!(updated["definition"]["actions"][0]["key"], "makeWidget");
    let (_, versions) = call(
        &app,
        "GET",
        &format!("/api/v1/integrations/{id}/versions"),
        &owner,
        None,
    )
    .await;
    assert_eq!(versions[0]["version"], 2);
    assert_eq!(versions[0]["sourceFormat"], "3.1.0");
    assert_eq!(versions[1]["apiVersion"], "1.0");
    assert_eq!(versions[1]["actions"], 2);
    assert_eq!(versions[1]["warnings"], 2);

    let (_, list) = call(&app, "GET", "/api/v1/integrations", &owner, None).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == id.as_str())
    );
}

#[tokio::test]
async fn people_without_access_see_and_change_nothing() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let org = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (_, created) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": org, "spec": SPEC_V1 })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();

    let stranger = login(&app, &user(&pools, false).await.1).await;
    let uri = format!("/api/v1/integrations/{id}");
    assert_eq!(
        call(&app, "GET", &uri, &stranger, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = call(&app, "GET", "/api/v1/integrations", &stranger, None).await;
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == id.as_str())
    );
    let import = json!({ "orgId": org, "key": "mine", "spec": SPEC_V1 });
    // An org they can't see doesn't exist for them.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &stranger,
        Some(import),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn updates_are_previewed_applied_and_rolled_back() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let org = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (_, created) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": org, "key": "widgets", "spec": SPEC_V1 })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    let base = format!("/api/v1/integrations/{id}");

    // V2 drops both listWidgets actions and the API key: breaking.
    let (status, preview) = call(
        &app,
        "POST",
        &format!("{base}/preview"),
        &owner,
        Some(json!({ "spec": SPEC_V2 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["baseVersion"], 1);
    assert_eq!(preview["diff"]["breaking"], true);
    assert_eq!(preview["diff"]["added"], json!(["makeWidget"]));
    assert_eq!(
        preview["diff"]["removed"],
        json!(["listWidgets", "listWidgets_2"])
    );
    assert!(
        preview["diff"]["connection"][0]["message"]
            .as_str()
            .unwrap()
            .contains("auth scheme key was removed")
    );
    // Previewing saves nothing.
    let (_, still) = call(&app, "GET", &base, &owner, None).await;
    assert_eq!(still["currentVersion"], 1);

    // Applying against a version that's no longer current is refused.
    let spec = format!("{base}/spec");
    let (status, _) = call(
        &app,
        "PUT",
        &spec,
        &owner,
        Some(json!({ "spec": SPEC_V2, "baseVersion": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, problem) = call(
        &app,
        "PUT",
        &spec,
        &owner,
        Some(json!({ "spec": SPEC_V1, "baseVersion": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");

    // Roll back to version 1; its actions are current again.
    let current = format!("{base}/current");
    let (status, rolled) = call(&app, "PUT", &current, &owner, Some(json!({ "version": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{rolled}");
    assert_eq!(rolled["currentVersion"], 1);
    assert_eq!(rolled["definition"]["actions"][0]["key"], "listWidgets");
    let (status, _) = call(&app, "PUT", &current, &owner, Some(json!({ "version": 9 }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The next import after a rollback doesn't reuse version 2.
    let (status, next) = call(&app, "PUT", &spec, &owner, Some(json!({ "spec": SPEC_V2 }))).await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["currentVersion"], 3);
}

fn caller(key: &str, action: &str) -> Value {
    json!({
        "apiVersion": "usehuxley.com/v1",
        "kind": "Workflow",
        "metadata": { "key": key, "name": format!("Caller {key}") },
        "spec": { "steps": [
            { "id": "list", "type": "integration.action", "action": action }
        ] }
    })
}

#[tokio::test]
async fn action_steps_are_checked_and_updates_show_who_they_break() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let org = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let (_, integration) = call(
        &app,
        "POST",
        "/api/v1/integrations",
        &owner,
        Some(json!({ "orgId": org, "key": "widgets", "spec": SPEC_V1 })),
    )
    .await;
    let integration_id = integration["id"].as_str().unwrap().to_owned();

    // A step naming an action that doesn't exist is a problem, and blocks
    // publishing.
    let (status, wrong) = call(
        &app,
        "POST",
        "/api/v1/workflows",
        &owner,
        Some(json!({ "orgId": org, "document": caller("wrong", "widgets/nope") })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wrong}");
    assert_eq!(wrong["diagnostics"][0]["path"], "spec.steps[0].action");
    let wrong_id = wrong["id"].as_str().unwrap();
    let (status, problem) = call(
        &app,
        "POST",
        &format!("/api/v1/workflows/{wrong_id}/publish"),
        &owner,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "spec.steps[0].action");

    // A correct one publishes.
    let (_, good) = call(
        &app,
        "POST",
        "/api/v1/workflows",
        &owner,
        Some(json!({ "orgId": org, "document": caller("good", "widgets/listWidgets") })),
    )
    .await;
    assert_eq!(good["diagnostics"], json!([]));
    let good_id = good["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workflows/{good_id}/publish"),
        &owner,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Removing listWidgets would break it: the preview says which step.
    let (_, preview) = call(
        &app,
        "POST",
        &format!("/api/v1/integrations/{integration_id}/preview"),
        &owner,
        Some(json!({ "spec": SPEC_V2 })),
    )
    .await;
    assert_eq!(preview["affected"][0]["workflowId"], good_id.as_str());
    assert_eq!(preview["affected"][0]["steps"][0]["stepId"], "list");
    assert_eq!(
        preview["affected"][0]["steps"][0]["problem"],
        "the action is removed"
    );

    // A rename (same method and path) breaks nothing: an alias keeps it.
    // (The second listWidgets, imported as listWidgets_2, becomes getWidget.)
    let at = SPEC_V1.rfind("operationId: listWidgets").unwrap();
    let renamed = format!(
        "{}operationId: getWidget{}",
        &SPEC_V1[..at],
        &SPEC_V1[at + "operationId: listWidgets".len()..]
    );
    let (_, preview) = call(
        &app,
        "POST",
        &format!("/api/v1/integrations/{integration_id}/preview"),
        &owner,
        Some(json!({ "spec": renamed })),
    )
    .await;
    assert_eq!(preview["diff"]["renamed"][0]["from"], "listWidgets_2");
    assert_eq!(preview["diff"]["renamed"][0]["to"], "getWidget");
    assert_eq!(preview["diff"]["breaking"], false, "{preview}");
    assert_eq!(preview["affected"], json!([]));
}
