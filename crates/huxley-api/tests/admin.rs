//! Admin endpoints end to end over HTTP, against a real PostgreSQL.

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

const PASSWORD: &str = "admin api passphrase";

fn app(pools: &TestPools) -> axum::Router {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(pools.app.clone(), auth, crypto, config))
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

#[tokio::test]
async fn module_toggles_need_permission_and_gate_disabled_modules() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let nobody = login(&app, &user(&pools, false).await).await;

    let (status, modules) = call(&app, "GET", "/api/v1/modules", Some(&nobody), None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "any signed-in user may list modules"
    );
    assert!(
        modules
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["key"] == "servicedesk")
    );

    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/modules/knowledge",
        Some(&nobody),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call(
        &app,
        "PUT",
        "/api/v1/modules/cx",
        Some(&owner),
        Some(json!({ "enabled": true })),
    )
    .await;
    // Unless another test left them on, cx's dependencies are missing.
    if status == StatusCode::CONFLICT {
        assert_eq!(
            body["type"],
            "https://usehuxley.com/problems/module-dependencies"
        );
    }
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/modules/not-a-module",
        Some(&owner),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unreleased modules are never active, so the gate rejects them.
    let state = AppState::new(
        pools.app.clone(),
        AuthService::new(
            pools.app.clone(),
            AuthConfig::default(),
            Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
        ),
        Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
        ApiConfig::default(),
    );
    let gate = huxley_api::module_routes::require_module(&state, "servicedesk").await;
    let err = gate.expect_err("unreleased module must be gated");
    assert_eq!(err.kind, "https://usehuxley.com/problems/module-disabled");
}

fn base32_decode(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut buffer, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let value = u32::try_from(ALPHABET.iter().position(|&a| a == c).unwrap()).unwrap();
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).unwrap());
        }
    }
    out
}

fn totp_now(secret: &[u8], offset: i64) -> String {
    use huxley_auth::totp::{code_at_step, step_at};
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    code_at_step(secret, step_at(now) + offset)
}

/// POSTs JSON without a session; returns status, the session cookie if one
/// was set, and the body.
async fn post_anonymous(
    app: &axum::Router,
    uri: &str,
    body: Value,
) -> (StatusCode, Option<String>, Value) {
    let req = Request::post(uri)
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get("set-cookie")
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_owned());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        cookie,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A non-owner with the Admin role at the root. Returns (email, id, cookie).
async fn admin_user(
    app: &axum::Router,
    pools: &TestPools,
    owner: &str,
    root: uuid::Uuid,
) -> (String, String, String) {
    let email = user(pools, false).await;
    let (_, roles) = call(app, "GET", "/api/v1/roles", Some(owner), None).await;
    let admin_role = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "admin")
        .unwrap()["id"]
        .clone();
    let cookie = login(app, &email).await;
    let (_, me) = call(app, "GET", "/api/v1/auth/me", Some(&cookie), None).await;
    let id = me["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        app,
        "POST",
        "/api/v1/role-assignments",
        Some(owner),
        Some(json!({ "userId": id, "roleId": admin_role, "orgId": root, "inherit": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    (email, id, cookie)
}

/// Sets up TOTP for the signed-in user and returns the secret.
async fn enroll_totp(app: &axum::Router, cookie: &str) -> Vec<u8> {
    let (status, mfa) = call(app, "GET", "/api/v1/auth/mfa", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mfa["totpEnabled"], false);
    let wrong = json!({ "password": "wrong password!!" });
    let (status, err) = call(
        app,
        "POST",
        "/api/v1/auth/mfa/totp",
        Some(cookie),
        Some(wrong),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    let right = json!({ "password": PASSWORD });
    let (status, enrollment) = call(
        app,
        "POST",
        "/api/v1/auth/mfa/totp",
        Some(cookie),
        Some(right),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enrollment}");
    let secret = base32_decode(enrollment["secret"].as_str().unwrap());
    let code = json!({ "code": totp_now(&secret, 0) });
    let (status, codes) = call(
        app,
        "POST",
        "/api/v1/auth/mfa/totp/confirm",
        Some(cookie),
        Some(code),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{codes}");
    assert_eq!(codes["recoveryCodes"].as_array().unwrap().len(), 10);
    secret
}

#[tokio::test]
async fn mfa_sign_in_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let (email, _, cookie) = admin_user(&app, &pools, &owner, root).await;
    let secret = enroll_totp(&app, &cookie).await;

    // Password sign-in now returns a challenge and no cookie.
    let credentials = json!({ "email": email, "password": PASSWORD });
    let (status, cookie, body) = post_anonymous(&app, "/api/v1/auth/login", credentials).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.is_none());
    assert_eq!(body["status"], "mfaRequired");
    assert_eq!(body["methods"], json!(["totp", "recoveryCode"]));
    let challenge = body["challenge"].as_str().unwrap().to_owned();

    let mfa = "/api/v1/auth/login/mfa";
    let (status, _, _) = post_anonymous(&app, mfa, json!({ "challenge": challenge })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let wrong = json!({ "challenge": challenge, "code": "000000" });
    let (status, _, err) = post_anonymous(&app, mfa, wrong).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        err["type"],
        "https://usehuxley.com/problems/invalid-mfa-code"
    );

    let right = json!({ "challenge": challenge, "code": totp_now(&secret, 1) });
    let (status, cookie, body) = post_anonymous(&app, mfa, right).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "signedIn");
    assert_eq!(body["user"]["email"], email.as_str());
    let (status, _) = call(&app, "GET", "/api/v1/auth/me", cookie.as_deref(), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn admin_mfa_reset_protects_owners() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let (email, admin_id, admin) = admin_user(&app, &pools, &owner, root).await;
    enroll_totp(&app, &admin).await;

    // The admin can't reset an owner's MFA, or their own through this route.
    let (_, owner_me) = call(&app, "GET", "/api/v1/auth/me", Some(&owner), None).await;
    let owner_id = owner_me["id"].as_str().unwrap();
    let uri = format!("/api/v1/users/{owner_id}/mfa");
    let (status, _) = call(&app, "DELETE", &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let uri = format!("/api/v1/users/{admin_id}/mfa");
    let (status, _) = call(&app, "DELETE", &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The owner resets the admin's MFA, which also signs the admin out.
    let (status, _) = call(&app, "DELETE", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    login(&app, &email).await;
}

#[tokio::test]
async fn passkey_endpoints_without_a_public_url() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let (status, methods) = call(&app, "GET", "/api/v1/auth/methods", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(methods["passkeys"], false);

    let (status, _, err) =
        post_anonymous(&app, "/api/v1/auth/login/passkey/options", json!({})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{err}");

    let cookie = login(&app, &user(&pools, false).await).await;
    let (status, list) = call(&app, "GET", "/api/v1/auth/passkeys", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));
    let body = json!({ "password": PASSWORD });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/passkeys/options",
        Some(&cookie),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // Two answers at once are refused before anything else happens.
    let both = json!({ "challenge": "hxm_x", "code": "123456", "recoveryCode": "AAAAA-AAAAA" });
    let (status, _, _) = post_anonymous(&app, "/api/v1/auth/login/mfa", both).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn mfa_policy_blocks_until_a_second_factor_is_set_up() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;

    // A role of its own, so flagging it can't affect other tests.
    let role_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO roles (id, key, name, description, is_system, permissions) \
         VALUES ($1, $2, $3, 'test', false, '{org:read}')",
    )
    .bind(role_id)
    .bind(format!("mfa_{}", role_id.simple()))
    .bind(unique("Mfa role"))
    .execute(&pools.owner)
    .await
    .unwrap();
    let email = user(&pools, false).await;
    let member = login(&app, &email).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    let member_id = me["id"].as_str().unwrap().to_owned();
    let grant = json!({ "userId": member_id, "roleId": role_id, "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(&app, "GET", "/api/v1/orgs", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);

    // Only policy managers can change policy.
    let uri = format!("/api/v1/security/mfa-policy/roles/{role_id}");
    let on = json!({ "required": true });
    let (status, _) = call(&app, "PUT", &uri, Some(&member), Some(on.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "PUT", &uri, Some(&owner), Some(on)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, policy) = call(
        &app,
        "GET",
        "/api/v1/security/mfa-policy",
        Some(&owner),
        None,
    )
    .await;
    assert!(
        policy["roleIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == &json!(role_id))
    );

    // The member's existing session is now limited to setting up MFA.
    let (status, err) = call(&app, "GET", "/api/v1/orgs", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        err["type"],
        "https://usehuxley.com/problems/mfa-setup-required"
    );
    let (status, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["mfaSetupRequired"], true);
    let (status, _) = call(&app, "GET", "/api/v1/auth/sessions", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);

    // Setting up an authenticator lifts the block.
    enroll_totp(&app, &member).await;
    let (status, _) = call(&app, "GET", "/api/v1/orgs", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    assert_eq!(me["mfaSetupRequired"], false);

    // Owners aren't covered by this role, so they're unaffected throughout.
    let (status, me) = call(&app, "GET", "/api/v1/auth/me", Some(&owner), None).await;
    assert_eq!(
        (status, me["mfaSetupRequired"].clone()),
        (StatusCode::OK, json!(false))
    );
    let off = json!({ "required": false });
    let (status, _) = call(&app, "PUT", &uri, Some(&owner), Some(off)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// A request authenticated only with `Authorization: Bearer` (no cookie, no
/// anti-forgery header), like a script would send.
async fn call_bearer(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
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

#[tokio::test]
async fn personal_tokens_act_as_their_owner_narrowed_to_scopes() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;

    // People's tokens must expire, within a year.
    let never = json!({ "name": "ci", "scopes": ["org:read"] });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/tokens",
        Some(&owner),
        Some(never),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let too_long = json!({ "name": "ci", "scopes": ["org:read"], "expiresInDays": 400 });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/tokens",
        Some(&owner),
        Some(too_long),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let unknown = json!({ "name": "ci", "scopes": ["org:explode"], "expiresInDays": 30 });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/auth/tokens",
        Some(&owner),
        Some(unknown),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let body = json!({ "name": "reporting", "scopes": ["org:read"], "expiresInDays": 30 });
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/auth/tokens",
        Some(&owner),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["tokenSecret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("hxp_"));
    assert!(token.starts_with(created["hint"].as_str().unwrap()));

    // In scope: works. Out of scope: refused, even though the owner could.
    let (status, _) = call_bearer(&app, "GET", "/api/v1/orgs", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_bearer(&app, "GET", "/api/v1/users", &token, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // State changes need no anti-forgery header with a bearer token; this
    // one is refused by scope, not CSRF.
    let new_user = json!({ "email": "scoped@example.com", "displayName": "Scoped" });
    let (status, err) = call_bearer(&app, "POST", "/api/v1/users", &token, Some(new_user)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err["type"], "https://usehuxley.com/problems/forbidden");

    // Tokens can't use self-service routes, so a leaked token can't take
    // over the account (change the password, mint more tokens, add MFA).
    let (status, _) = call_bearer(&app, "GET", "/api/v1/auth/me", &token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let more = json!({ "name": "x", "scopes": ["*"], "expiresInDays": 1 });
    let (status, _) = call_bearer(&app, "POST", "/api/v1/auth/tokens", &token, Some(more)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Listed with usage; never with the secret.
    let (_, list) = call(&app, "GET", "/api/v1/auth/tokens", Some(&owner), None).await;
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == created["id"])
        .unwrap();
    assert!(listed["lastUsedAt"].is_string());
    assert!(!list.to_string().contains(&token));

    // Revoked: rejected.
    let uri = format!("/api/v1/auth/tokens/{}", created["id"].as_str().unwrap());
    let (status, _) = call(&app, "DELETE", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, err) = call_bearer(&app, "GET", "/api/v1/orgs", &token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(err["type"], "https://usehuxley.com/problems/invalid-token");
}

#[tokio::test]
async fn service_accounts_use_tokens_and_roles_but_cannot_sign_in() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let nobody = login(&app, &user(&pools, false).await).await;

    let (status, _) = call(&app, "GET", "/api/v1/service-accounts", Some(&nobody), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let body = json!({ "name": "RMM sync", "description": "Pulls devices nightly" });
    let (status, sa) = call(
        &app,
        "POST",
        "/api/v1/service-accounts",
        Some(&owner),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sa}");
    assert_eq!(sa["kind"], "service");
    let sa_id = sa["id"].as_str().unwrap().to_owned();
    let sa_email = sa["email"].as_str().unwrap().to_owned();
    assert!(sa_email.ends_with("@service.invalid"));

    // Roles work as for people, except Owner.
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let role = |key: &str| {
        roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == key)
            .unwrap()["id"]
            .clone()
    };
    let grant = json!({ "userId": sa_id, "roleId": role("technician"), "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let owner_grant = json!({ "userId": sa_id, "roleId": role("owner"), "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(owner_grant),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Its tokens may be non-expiring.
    let uri = format!("/api/v1/service-accounts/{sa_id}/tokens");
    let body = json!({ "name": "prod", "scopes": ["*"] });
    let (status, created) = call(&app, "POST", &uri, Some(&owner), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(created["expiresAt"].is_null());
    let token = created["tokenSecret"].as_str().unwrap().to_owned();
    let (status, _) = call_bearer(&app, "GET", "/api/v1/users", &token, None).await;
    assert_eq!(status, StatusCode::OK, "technician can read users");
    let new_user = json!({ "email": "by-sa@example.com", "displayName": "By SA" });
    let (status, _) = call_bearer(&app, "POST", "/api/v1/users", &token, Some(new_user)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "but not create them");

    // No interactive sign-in, even with a guessed password.
    let creds = json!({ "email": sa_email, "password": PASSWORD });
    let (status, _, _) = post_anonymous(&app, "/api/v1/auth/login", creds).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Disabling the account stops its tokens.
    let uri = format!("/api/v1/service-accounts/{sa_id}");
    let (status, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&owner),
        Some(json!({ "status": "disabled" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_bearer(&app, "GET", "/api/v1/orgs", &token, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // People aren't service accounts.
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&nobody), None).await;
    let uri = format!("/api/v1/service-accounts/{}", me["id"].as_str().unwrap());
    let (status, _) = call(&app, "GET", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn team_roles_reach_members_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let member_email = user(&pools, false).await;
    let member = login(&app, &member_email).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    let member_id = me["id"].as_str().unwrap().to_owned();

    // Only team managers create teams.
    let body = json!({ "name": unique("Service desk"), "description": "Tier 1" });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/teams",
        Some(&member),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, team) = call(&app, "POST", "/api/v1/teams", Some(&owner), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{team}");
    let team_id = team["id"].as_str().unwrap().to_owned();

    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let role = |key: &str| {
        roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == key)
            .unwrap()["id"]
            .clone()
    };
    let grant = json!({ "teamId": team_id, "roleId": role("read_only"), "orgId": root });
    let (status, assignment) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{assignment}");
    assert_eq!(assignment["teamId"], json!(team_id));
    // Joining the team grants Read-only; leaving takes it away.
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let uri = format!("/api/v1/teams/{team_id}/members/{member_id}");
    let (status, _) = call(&app, "PUT", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, detail) = call(
        &app,
        "GET",
        &format!("/api/v1/teams/{team_id}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["memberCount"], 1);
    assert_eq!(detail["members"][0]["email"], member_email.as_str());
    assert_eq!(detail["assignments"][0]["roleKey"], "read_only");
    let (_, of_user) = call(
        &app,
        "GET",
        &format!("/api/v1/teams/of-user/{member_id}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(of_user[0]["id"], json!(team_id));

    let (status, _) = call(&app, "DELETE", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/teams/{team_id}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn team_assignments_are_validated() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let member = login(&app, &user(&pools, false).await).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    let member_id = me["id"].as_str().unwrap().to_owned();
    let body = json!({ "name": unique("Checks") });
    let (_, team) = call(&app, "POST", "/api/v1/teams", Some(&owner), Some(body)).await;
    let team_id = team["id"].as_str().unwrap().to_owned();
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let role = |key: &str| {
        roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == key)
            .unwrap()["id"]
            .clone()
    };
    let both = json!({ "teamId": team_id, "userId": member_id, "roleId": role("read_only"), "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(both),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let owner_grant = json!({ "teamId": team_id, "roleId": role("owner"), "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(owner_grant),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}
