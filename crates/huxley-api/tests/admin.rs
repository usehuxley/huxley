//! Admin endpoints end to end over HTTP, against a real PostgreSQL.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;

const PASSWORD: &str = "admin api passphrase";

fn app(pools: &TestPools) -> axum::Router {
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(pools.app.clone(), auth, config))
}

async fn user(pools: &TestPools, owner: bool) -> String {
    let email = format!("{}@example.com", unique("api").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Api User", PASSWORD)
        .await
        .unwrap();
    if owner {
        huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    email
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-requested-with", "huxley");
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
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

#[tokio::test]
async fn owner_manages_orgs_users_and_roles() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;

    let (status, me) = call(&app, "GET", "/api/v1/auth/me", Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["isOwner"], true);
    assert!(
        me["grantedPermissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "user:create")
    );

    let name = unique("Client");
    let (status, org) = call(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&owner),
        Some(json!({ "parentId": root, "kind": "client", "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org}");
    let org_id = org["id"].as_str().unwrap().to_owned();

    let (status, renamed) = call(
        &app,
        "PATCH",
        &format!("/api/v1/orgs/{org_id}"),
        Some(&owner),
        Some(json!({ "name": format!("{name} renamed"), "version": org["version"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/orgs/{org_id}"),
        Some(&owner),
        Some(json!({ "name": "stale", "version": org["version"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale version is rejected");
    assert_eq!(renamed["version"], org["version"].as_i64().unwrap() + 1);

    let email = format!("{}@example.com", unique("new").replace(' ', "."));
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/users",
        Some(&owner),
        Some(json!({ "email": email, "displayName": "New Tech" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(created["generatedPassword"].as_str().unwrap().len() >= 20);

    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let technician = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    let (status, assignment) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(json!({ "userId": created["user"]["id"], "roleId": technician, "orgId": org_id })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{assignment}");

    let (status, audit) = call(
        &app,
        "GET",
        "/api/v1/audit?action=role.",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["details"]["assignmentId"] == assignment["id"])
    );
}

#[tokio::test]
async fn scoped_users_see_only_their_orgs_and_cannot_administer() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;

    let (_, mine) = call(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&owner),
        Some(json!({ "parentId": root, "kind": "client", "name": unique("Mine") })),
    )
    .await;
    let (_, theirs) = call(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&owner),
        Some(json!({ "parentId": root, "kind": "client", "name": unique("Theirs") })),
    )
    .await;

    let tech_email = user(&pools, false).await;
    let tech_session = login(&app, &tech_email).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&tech_session), None).await;
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let technician = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(json!({ "userId": me["id"], "roleId": technician, "orgId": mine["id"] })),
    )
    .await;

    let (status, orgs) = call(&app, "GET", "/api/v1/orgs", Some(&tech_session), None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&Value> = orgs.as_array().unwrap().iter().map(|o| &o["id"]).collect();
    assert!(ids.contains(&&mine["id"]));
    assert!(!ids.contains(&&theirs["id"]), "out-of-scope org leaked");

    // Reading an invisible org is a 404, not a 403: existence isn't revealed.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/orgs/{}", theirs["id"].as_str().unwrap()),
        Some(&tech_session),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Visible but not permitted: 403.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&tech_session),
        Some(json!({ "parentId": mine["id"], "kind": "client", "name": "Nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // No instance-level permissions.
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&tech_session), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn disabling_a_user_ends_their_sessions() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let victim_email = user(&pools, false).await;
    let victim = login(&app, &victim_email).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&victim), None).await;

    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/users/{}", me["id"].as_str().unwrap()),
        Some(&owner),
        Some(json!({ "status": "disabled" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/api/v1/auth/me", Some(&victim), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
