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
            "https://usehuxley.io/problems/module-dependencies"
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
    let gate = huxley_api::module_routes::require_module(&state, "contracts").await;
    let err = gate.expect_err("unreleased module must be gated");
    assert_eq!(err.kind, "https://usehuxley.io/problems/module-disabled");
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
        "https://usehuxley.io/problems/invalid-mfa-code"
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
        "https://usehuxley.io/problems/mfa-setup-required"
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
    assert_eq!(err["type"], "https://usehuxley.io/problems/forbidden");

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
    assert_eq!(err["type"], "https://usehuxley.io/problems/invalid-token");
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

/// An app whose emails land in memory, with a public URL for links.
fn app_with_mail(
    pools: &TestPools,
) -> (
    axum::Router,
    std::sync::Arc<std::sync::Mutex<Vec<huxley_email::Email>>>,
) {
    let (mailer, outbox) = huxley_email::Mailer::memory();
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(
        pools.app.clone(),
        AuthConfig {
            public_url: Some("https://huxley.example.com".into()),
            ..AuthConfig::default()
        },
        crypto.clone(),
    )
    .with_mailer(mailer);
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    (
        router(AppState::new(pools.app.clone(), auth, crypto, config)),
        outbox,
    )
}

async fn wait_for_mail(
    outbox: &std::sync::Arc<std::sync::Mutex<Vec<huxley_email::Email>>>,
    to: &str,
) -> huxley_email::Email {
    for _ in 0..50 {
        if let Some(m) = outbox.lock().unwrap().iter().find(|m| m.to == to) {
            return m.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no email to {to}");
}

fn link_token(email: &huxley_email::Email) -> String {
    let start = email.text.find("?token=").unwrap() + "?token=".len();
    email.text[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn password_reset_and_invitations_over_http() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let (app, outbox) = app_with_mail(&pools);
    let (_, methods) = call(&app, "GET", "/api/v1/auth/methods", None, None).await;
    assert_eq!(methods["passwordReset"], true);

    // Reset: same answer for known and unknown addresses.
    let email = user(&pools, false).await;
    let (status, _, _) = post_anonymous(
        &app,
        "/api/v1/auth/password-reset",
        json!({ "email": email }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let unknown = json!({ "email": "nobody-here-at-all@example.com" });
    let (status, _, _) = post_anonymous(&app, "/api/v1/auth/password-reset", unknown).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let token = link_token(&wait_for_mail(&outbox, &email).await);
    let confirm = json!({ "token": token, "newPassword": "a fresh new passphrase" });
    let (status, _, _) =
        post_anonymous(&app, "/api/v1/auth/password-reset/confirm", confirm.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, err) =
        post_anonymous(&app, "/api/v1/auth/password-reset/confirm", confirm).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["type"], "https://usehuxley.io/problems/invalid-link");
    let creds = json!({ "email": email, "password": "a fresh new passphrase" });
    let (status, cookie, _) = post_anonymous(&app, "/api/v1/auth/login", creds).await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", cookie.as_deref(), None).await;
    assert_eq!(me["emailVerified"], true);

    // Invitations: no password shown; the invitee sets one.
    let owner = login(&app, &user(&pools, true).await).await;
    let invitee = format!("{}@example.com", unique("invitee").replace(' ', "."));
    let body = json!({ "email": invitee, "displayName": "Ivy Invitee", "invite": true });
    let (status, created) = call(&app, "POST", "/api/v1/users", Some(&owner), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["invited"], true);
    assert!(created["generatedPassword"].is_null());
    let mail = wait_for_mail(&outbox, &invitee).await;
    assert!(mail.subject.contains("invited"));
    let confirm = json!({ "token": link_token(&mail), "newPassword": "my own chosen passphrase" });
    let (status, _, _) = post_anonymous(&app, "/api/v1/auth/password-reset/confirm", confirm).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn email_features_report_when_email_is_off() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let (_, methods) = call(&app, "GET", "/api/v1/auth/methods", None, None).await;
    assert_eq!(methods["passwordReset"], false);
    let (status, _, _) = post_anonymous(
        &app,
        "/api/v1/auth/password-reset",
        json!({ "email": "a@example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let owner = login(&app, &user(&pools, true).await).await;
    let body = json!({ "email": "inv@example.com", "displayName": "Inv", "invite": true });
    let (status, _) = call(&app, "POST", "/api/v1/users", Some(&owner), Some(body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn notifications_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let member = login(&app, &user(&pools, false).await).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    let member_id = me["id"].as_str().unwrap().to_owned();

    // Being given a role notifies you.
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let tech = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    let grant = json!({ "userId": member_id, "roleId": tech, "orgId": root });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, feed) = call(&app, "GET", "/api/v1/notifications", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(feed["unreadCount"], 1);
    assert_eq!(feed["items"][0]["kind"], "access.role_granted");
    assert_eq!(
        feed["items"][0]["title"],
        "You were given the Technician role"
    );

    // The stream opens with the unread count.
    let req = Request::get("/api/v1/notifications/stream")
        .header("cookie", &member)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    let mut body = res.into_body();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(
        text.contains("event: unread") && text.contains("data: 1"),
        "{text}"
    );

    // Mark read; someone else's id is unknown to you.
    let id = feed["items"][0]["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/notifications/{id}/read"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/notifications/{id}/read"),
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, count) = call(
        &app,
        "GET",
        "/api/v1/notifications/unread-count",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(count["unreadCount"], 0);
}

#[tokio::test]
async fn notification_preferences_over_http() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let member = login(&app, &user(&pools, false).await).await;
    // Preferences: optional kinds can change; security notices can't.
    let (_, prefs) = call(
        &app,
        "GET",
        "/api/v1/notifications/preferences",
        Some(&member),
        None,
    )
    .await;
    assert!(
        prefs
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "security.password_changed" && p["mandatory"] == true)
    );
    let both = json!({ "inApp": true, "email": true });
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/notifications/preferences/access.role_granted",
        Some(&member),
        Some(both.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let off = json!({ "inApp": false, "email": false });
    let (status, _) = call(
        &app,
        "PUT",
        "/api/v1/notifications/preferences/security.mfa_changed",
        Some(&member),
        Some(off),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn custom_fields_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let (_, client) = call(
        &app,
        "POST",
        "/api/v1/orgs",
        Some(&owner),
        Some(json!({ "parentId": root, "kind": "client", "name": unique("Fieldco") })),
    )
    .await;
    let client_id = client["id"].as_str().unwrap().to_owned();

    // A reader (technician at the client) and an outsider.
    let reader_email = user(&pools, false).await;
    let reader = login(&app, &reader_email).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&reader), None).await;
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let tech = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    let grant = json!({ "userId": me["id"], "roleId": tech, "orgId": client_id });
    call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    let outsider = login(&app, &user(&pools, false).await).await;

    // Only field managers define fields. (Key is unique per run.)
    let key = format!("seats_{}", uuid::Uuid::now_v7().simple());
    let def = json!({ "entityType": "organization", "key": key, "label": "Seats", "fieldType": "number" });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/custom-fields",
        Some(&reader),
        Some(def.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, field) = call(
        &app,
        "POST",
        "/api/v1/custom-fields",
        Some(&owner),
        Some(def),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    let uri = format!("/api/v1/entities/organization/{client_id}/fields");

    // Owners set values; the reader can read but not write; outsiders see nothing.
    let body = json!({ "values": { key.as_str(): 40 } });
    let (status, values) = call(&app, "PATCH", &uri, Some(&owner), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{values}");
    let (status, values) = call(&app, "GET", &uri, Some(&reader), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        values
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["key"] == key.as_str())
            .unwrap()["value"],
        40
    );
    let (status, _) = call(&app, "PATCH", &uri, Some(&reader), Some(body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "GET", &uri, Some(&outsider), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let bad = json!({ "values": { key.as_str(): "forty" } });
    let (status, err) = call(&app, "PATCH", &uri, Some(&owner), Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["errors"][0]["pointer"], format!("/{key}"));

    // Clean up the definition so other tests don't see it.
    let archive = format!(
        "/api/v1/custom-fields/{}/archive",
        field["id"].as_str().unwrap()
    );
    let (status, _) = call(
        &app,
        "POST",
        &archive,
        Some(&owner),
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn tags_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let body = json!({ "parentId": root, "kind": "client", "name": unique("Tagco") });
    let (_, client) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let client_id = client["id"].as_str().unwrap().to_owned();
    let reader = login(&app, &user(&pools, false).await).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&reader), None).await;
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let tech = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    let grant = json!({ "userId": me["id"], "roleId": tech, "orgId": client_id });
    call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;

    // Tags.
    let (status, tag) = call(
        &app,
        "POST",
        "/api/v1/tags",
        Some(&owner),
        Some(json!({ "name": unique("Managed"), "color": "success" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let tag_uri = format!(
        "/api/v1/entities/organization/{client_id}/tags/{}",
        tag["id"].as_str().unwrap()
    );
    let (status, _) = call(&app, "PUT", &tag_uri, Some(&reader), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "PUT", &tag_uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, on) = call(
        &app,
        "GET",
        &format!("/api/v1/entities/organization/{client_id}/tags"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(on[0]["id"], tag["id"]);
}

/// A multipart upload of one `file` part.
async fn upload(
    app: &axum::Router,
    uri: &str,
    cookie: &str,
    name: &str,
    content_type: &str,
    data: &[u8],
) -> (StatusCode, Value) {
    let boundary = "huxleyboundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n"
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
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn attachments_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let body = json!({ "parentId": root, "kind": "client", "name": unique("Fileco") });
    let (_, client) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let client_id = client["id"].as_str().unwrap().to_owned();
    let reader = login(&app, &user(&pools, false).await).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&reader), None).await;
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let tech = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .clone();
    let grant = json!({ "userId": me["id"], "roleId": tech, "orgId": client_id });
    call(
        &app,
        "POST",
        "/api/v1/role-assignments",
        Some(&owner),
        Some(grant),
    )
    .await;
    let outsider = login(&app, &user(&pools, false).await).await;
    let uri = format!("/api/v1/entities/organization/{client_id}/attachments");

    // Readers can't upload; the owner can. An HTML upload is stored as such…
    let page = b"<script>alert(1)</script>";
    let (status, _) = upload(&app, &uri, &reader, "x.html", "text/html", page).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, file) = upload(&app, &uri, &owner, "../evil page.html", "text/html", page).await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    assert_eq!(file["filename"], "evil page.html");
    let id = file["id"].as_str().unwrap().to_owned();

    // …but downloads as an inert attachment, never inline HTML.
    let req = Request::get(format!("/api/v1/attachments/{id}/content"))
        .header("cookie", &reader)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "application/octet-stream");
    assert!(
        h["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert!(
        h["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(
        &res.into_body().collect().await.unwrap().to_bytes()[..],
        page
    );

    // Outsiders can't see it; readers can't delete it.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/attachments/{id}/content"),
        Some(&outsider),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "GET", &uri, Some(&outsider), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/attachments/{id}"),
        Some(&reader),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, list) = call(&app, "GET", &uri, Some(&reader), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/attachments/{id}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "GET", &uri, Some(&reader), None).await;
    assert_eq!(list, json!([]));
}

#[tokio::test]
async fn uploads_over_the_limit_are_refused() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let state = AppState::new(pools.app.clone(), auth, crypto, config)
        .with_storage(huxley_storage::Storage::memory().with_max_upload_bytes(1024));
    let app = router(state);
    let owner = login(&app, &user(&pools, true).await).await;
    let uri = format!("/api/v1/entities/organization/{root}/attachments");
    let (status, err) = upload(
        &app,
        &uri,
        &owner,
        "big.bin",
        "application/octet-stream",
        &[7u8; 4096],
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{err}");
}

#[tokio::test]
async fn custom_roles_and_the_explainer_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let member = login(&app, &user(&pools, false).await).await;
    let (_, me) = call(&app, "GET", "/api/v1/auth/me", Some(&member), None).await;
    let member_id = me["id"].as_str().unwrap().to_owned();

    let body = json!({ "name": unique("Auditor"), "permissions": ["audit:read", "org:read"] });
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/roles",
        Some(&member),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, role) = call(&app, "POST", "/api/v1/roles", Some(&owner), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    assert_eq!(role["isSystem"], false);
    let role_id = role["id"].as_str().unwrap().to_owned();
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
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&member), None).await;
    assert_eq!(status, StatusCode::OK);

    // Why can they read the audit log? The explainer says.
    let uri = format!("/api/v1/users/{member_id}/effective-permissions");
    let (status, explained) = call(&app, "GET", &uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK);
    let audit = explained
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "audit:read")
        .unwrap();
    assert_eq!(audit["allowed"], true);
    assert_eq!(audit["via"][0]["roleName"], role["name"]);

    // Editing the role changes their access at the next request.
    let narrower = json!({ "name": role["name"], "permissions": ["org:read"] });
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/roles/{role_id}"),
        Some(&owner),
        Some(narrower),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/api/v1/audit", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Still assigned: can't delete. System roles: can't edit.
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/roles/{role_id}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, roles) = call(&app, "GET", "/api/v1/roles", Some(&owner), None).await;
    let tech = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "technician")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let edit = json!({ "name": "Tech", "permissions": ["org:read"] });
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/roles/{tech}"),
        Some(&owner),
        Some(edit),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

async fn raw_get(
    app: &axum::Router,
    uri: &str,
    cookie: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let req = Request::get(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn audit_export_streams_every_event_in_order() {
    let Some(pools) = pools().await else { return };
    ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let member = login(&app, &user(&pools, false).await).await;
    let (status, _, _) = raw_get(&app, "/api/v1/audit/export", &member).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM audit_events")
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    let (status, headers, body) = raw_get(&app, "/api/v1/audit/export", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/x-ndjson");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains(".jsonl")
    );
    let rows: Vec<Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // Every event (batches of 1000 join seamlessly), oldest first, no repeats.
    assert!(
        i64::try_from(rows.len()).unwrap() > before,
        "{} rows, {before} before",
        rows.len()
    );
    assert!(
        rows.windows(2)
            .all(|w| w[0]["seq"].as_i64() < w[1]["seq"].as_i64())
    );
    assert_eq!(rows[0]["hash"].as_str().unwrap().len(), 64);
    assert!(
        rows.iter().any(|r| r["action"] == "audit.exported"),
        "the export audits itself"
    );

    let (status, headers, body) =
        raw_get(&app, "/api/v1/audit/export?format=csv&action=auth.", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/csv")
    );
    let mut lines = body.lines();
    assert!(lines.next().unwrap().starts_with("seq,id,occurred_at,"));
    assert!(lines.all(|l| l.contains(",auth.")));

    let (status, _, body) = raw_get(
        &app,
        "/api/v1/audit/export?from=2999-01-01T00:00:00Z",
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "");
}

#[tokio::test]
async fn org_settings_inherit_and_lock_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let body = json!({ "parentId": root, "kind": "provider", "name": unique("Brandco") });
    let (_, msp) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let msp_id = msp["id"].as_str().unwrap().to_owned();
    let body = json!({ "parentId": msp_id, "kind": "client", "name": unique("Brand client") });
    let (_, client) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let client_id = client["id"].as_str().unwrap().to_owned();

    // A technician at the client can read settings but not change them.
    let tech = login(&app, &user(&pools, false).await).await;
    grant_role(&app, &owner, &tech, "technician", &client_id).await;

    let msp_uri = format!("/api/v1/orgs/{msp_id}/settings/branding.accent_color");
    let client_uri = format!("/api/v1/orgs/{client_id}/settings/branding.accent_color");
    let (status, _) = call(
        &app,
        "PUT",
        &msp_uri,
        Some(&owner),
        Some(json!({ "value": "#123456", "locked": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, list) = call(
        &app,
        "GET",
        &format!("/api/v1/orgs/{client_id}/settings"),
        Some(&tech),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let accent = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "branding.accent_color")
        .unwrap();
    assert_eq!(accent["value"], "#123456");
    assert_eq!(accent["source"]["id"], msp_id.as_str());
    assert_eq!(accent["lockedBy"]["id"], msp_id.as_str());
    assert_eq!(accent["kind"], "color");

    let (status, _) = call(
        &app,
        "PUT",
        &client_uri,
        Some(&tech),
        Some(json!({ "value": "#000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, err) = call(
        &app,
        "PUT",
        &client_uri,
        Some(&owner),
        Some(json!({ "value": "#000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    let prefix_uri = format!("/api/v1/orgs/{client_id}/settings/numbering.ticket_prefix");
    let (status, _) = call(
        &app,
        "PUT",
        &prefix_uri,
        Some(&owner),
        Some(json!({ "value": "lower" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &app,
        "PUT",
        &prefix_uri,
        Some(&owner),
        Some(json!({ "value": "CL" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/orgs/{client_id}/settings/nope.nope"),
        Some(&owner),
        Some(json!({ "value": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Clearing the MSP's value unlocks the client.
    let (status, _) = call(&app, "DELETE", &msp_uri, Some(&owner), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &app,
        "PUT",
        &client_uri,
        Some(&owner),
        Some(json!({ "value": "#000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// Gives the user behind `member` a system role at `org_id`.
async fn grant_role(app: &axum::Router, owner: &str, member: &str, role_key: &str, org_id: &str) {
    let (_, me) = call(app, "GET", "/api/v1/auth/me", Some(member), None).await;
    let (_, roles) = call(app, "GET", "/api/v1/roles", Some(owner), None).await;
    let role = roles
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == role_key)
        .unwrap()["id"]
        .clone();
    let grant = json!({ "userId": me["id"], "roleId": role, "orgId": org_id });
    let (status, _) = call(
        app,
        "POST",
        "/api/v1/role-assignments",
        Some(owner),
        Some(grant),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn modules_narrow_per_provider_org_over_http() {
    let Some(pools) = pools().await else { return };
    let root = ensure_root(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await).await;
    let nobody = login(&app, &user(&pools, false).await).await;
    let body = json!({ "parentId": root, "kind": "provider", "name": unique("Second MSP") });
    let (_, msp) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let msp_id = msp["id"].as_str().unwrap().to_owned();
    let body = json!({ "parentId": msp_id, "kind": "client", "name": unique("Its client") });
    let (_, client) = call(&app, "POST", "/api/v1/orgs", Some(&owner), Some(body)).await;
    let client_id = client["id"].as_str().unwrap().to_owned();

    let off = json!({ "enabled": false });
    let uri = format!("/api/v1/orgs/{msp_id}/modules/knowledge");
    let (status, _) = call(&app, "PUT", &uri, Some(&nobody), Some(off.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, after) = call(&app, "PUT", &uri, Some(&owner), Some(off.clone())).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    let knowledge = after
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["key"] == "knowledge")
        .unwrap();
    assert_eq!(knowledge["enabledHere"], false);
    if knowledge["enabled"] == true {
        assert_eq!(knowledge["narrowedBy"]["orgId"], msp_id.as_str());
    }

    // Clients and the root don't narrow; unknown modules are 404.
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/orgs/{client_id}/modules/knowledge"),
        Some(&owner),
        Some(off.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/orgs/{root}/modules/knowledge"),
        Some(&owner),
        Some(off.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/orgs/{msp_id}/modules/nope"),
        Some(&owner),
        Some(off),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Readers of the client see the state there.
    let (status, at_client) = call(
        &app,
        "GET",
        &format!("/api/v1/orgs/{client_id}/modules"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        at_client
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["key"] != "knowledge" || m["enabledHere"] == false)
    );
    let (status, _) = call(
        &app,
        "PUT",
        &uri,
        Some(&owner),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
