//! SCIM 2.0 provisioning (ADR-088) over HTTP, the way Entra ID and Okta
//! drive it: look a user up by userName, create them, change and deactivate
//! them, manage a group's members with PATCH, and delete. Against a real
//! PostgreSQL.

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

const PASSWORD: &str = "scim api passphrase";
const USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const PATCH: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

fn app(pools: &TestPools) -> axum::Router {
    let crypto = Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto.clone());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(pools.app.clone(), auth, crypto, config))
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    auth: (&str, &str),
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(auth.0, auth.1)
        .header("x-requested-with", "huxley");
    req = req.header("content-type", "application/scim+json");
    let req = req
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// An owner's session cookie, and a provider with SCIM on: (cookie, provider, token).
async fn setup(pools: &TestPools, app: &axum::Router) -> (String, Uuid, String) {
    ensure_root(pools).await;
    let email = format!("{}@example.com", unique("scimowner").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let owner = huxley_auth::create_user(&mut tx, &ctx, &email, "Owner", PASSWORD)
        .await
        .unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, owner.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::post("/api/v1/auth/login")
                .header("x-requested-with", "huxley")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": PASSWORD }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let provider: Uuid = sqlx::query_scalar(
        "INSERT INTO oidc_providers (id, key, name, preset, issuer_url, client_id) \
         VALUES (gen_random_uuid(), $1, 'Entra', 'entra', 'https://login.example', 'client') RETURNING id",
    )
    .bind(unique("entra").replace(' ', "-").to_lowercase())
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    let (status, token) = send(
        app,
        "POST",
        &format!("/api/v1/security/sso/providers/{provider}/scim/token"),
        ("cookie", &cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{token}");
    (
        cookie,
        provider,
        token["token"].as_str().unwrap().to_owned(),
    )
}

/// Provisions a user with just a userName; returns their id.
async fn provision(app: &axum::Router, scim: (&str, &str), user_name: &str) -> String {
    let (status, u) = send(
        app,
        "POST",
        "/scim/v2/Users",
        scim,
        Some(json!({ "schemas": [USER], "userName": user_name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{u}");
    u["id"].as_str().unwrap().to_owned()
}

/// A group's member ids, sorted.
async fn members(app: &axum::Router, scim: (&str, &str), group: &str) -> Vec<String> {
    let (_, g) = send(app, "GET", &format!("/scim/v2/Groups/{group}"), scim, None).await;
    let mut ids: Vec<String> = g["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["value"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn an_identity_provider_provisions_and_deprovisions_users() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (_, _, token) = setup(&pools, &app).await;
    let bearer = format!("Bearer {token}");
    let scim = ("authorization", bearer.as_str());
    let stamp = unique("u").replace(' ', ".").to_lowercase();
    let user_name = format!("{stamp}@contoso.example");

    // Without the token, nothing.
    let (status, body) = send(
        &app,
        "GET",
        "/scim/v2/Users",
        ("authorization", "Bearer hxscim_nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:Error"
    );
    let (status, _) = send(&app, "GET", "/scim/v2/ServiceProviderConfig", scim, None).await;
    assert_eq!(status, StatusCode::OK);

    // Entra looks the user up first, then creates them.
    let filter = format!("/scim/v2/Users?filter=userName%20eq%20%22{user_name}%22");
    let (_, found) = send(&app, "GET", &filter, scim, None).await;
    assert_eq!(found["totalResults"], 0);
    let (status, created) = send(
        &app,
        "POST",
        "/scim/v2/Users",
        scim,
        Some(json!({
            "schemas": [USER], "userName": user_name, "externalId": "obj-1", "active": true,
            "name": { "givenName": "Grace", "familyName": "Hopper" },
            "emails": [{ "value": user_name, "type": "work", "primary": true }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["displayName"], "Grace Hopper");
    let id = created["id"].as_str().unwrap().to_owned();
    let (_, found) = send(&app, "GET", &filter, scim, None).await;
    assert_eq!(found["totalResults"], 1);
    assert_eq!(found["Resources"][0]["externalId"], "obj-1");
    let (status, dup) = send(
        &app,
        "POST",
        "/scim/v2/Users",
        scim,
        Some(json!({ "schemas": [USER], "userName": user_name })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(dup["scimType"], "uniqueness");

    // A session they have ends when they're deactivated (Entra's PATCH).
    let session = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO sessions (id, user_id, token_hash, idle_expires_at, absolute_expires_at) \
         VALUES (gen_random_uuid(), $1, gen_random_bytes(32), now() + interval '1 day', now() + interval '1 day') RETURNING id",
    )
    .bind(Uuid::parse_str(&id).unwrap())
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    let (status, patched) = send(
        &app,
        "PATCH",
        &format!("/scim/v2/Users/{id}"),
        scim,
        Some(json!({ "schemas": [PATCH], "Operations": [
            { "op": "Replace", "path": "active", "value": "False" },
            { "op": "Replace", "path": "name.givenName", "value": "Amazing Grace" },
            { "op": "Replace", "path": "displayName", "value": "Amazing Grace Hopper" },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["active"], false);
    assert_eq!(patched["displayName"], "Amazing Grace Hopper");
    assert_eq!(patched["name"]["givenName"], "Amazing Grace");
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM sessions WHERE id = $1")
            .bind(session)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert!(revoked, "deactivating ends their sessions");
    // Okta's style: reactivate with a pathless replace.
    let (_, patched) = send(
        &app,
        "PATCH",
        &format!("/scim/v2/Users/{id}"),
        scim,
        Some(json!({ "schemas": [PATCH], "Operations": [{ "op": "replace", "value": { "active": true } }] })),
    )
    .await;
    assert_eq!(patched["active"], true);
}

#[tokio::test]
async fn existing_users_are_adopted_and_deleting_deprovisions() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (cookie, provider, token) = setup(&pools, &app).await;
    let bearer = format!("Bearer {token}");
    let scim = ("authorization", bearer.as_str());
    let stamp = unique("d").replace(' ', ".").to_lowercase();
    let id = provision(&app, scim, &format!("{stamp}@contoso.example")).await;

    // An existing Huxley user with the same email is adopted, not duplicated.
    let existing_email = format!("{stamp}.existing@contoso.example");
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let existing = huxley_auth::create_user(&mut tx, &ctx, &existing_email, "Existing", PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, adopted) = send(
        &app,
        "POST",
        "/scim/v2/Users",
        scim,
        Some(json!({ "schemas": [USER], "userName": existing_email, "displayName": "Existing Person" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{adopted}");
    assert_eq!(adopted["id"], existing.id.to_string());

    // Deleting deprovisions: disabled and unlinked, but the user stays.
    let (status, _) = send(&app, "DELETE", &format!("/scim/v2/Users/{id}"), scim, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", &format!("/scim/v2/Users/{id}"), scim, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let status: String = sqlx::query_scalar("SELECT status::text FROM users WHERE id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(status, "disabled");

    // The admin view counts what's provisioned; turning SCIM off stops the token.
    let (_, settings) = send(
        &app,
        "GET",
        &format!("/api/v1/security/sso/providers/{provider}/scim"),
        ("cookie", &cookie),
        None,
    )
    .await;
    assert_eq!(settings["enabled"], true);
    assert_eq!(settings["users"], 1);
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/security/sso/providers/{provider}/scim"),
        ("cookie", &cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", "/scim/v2/Users", scim, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn groups_become_teams_whose_members_the_provider_manages() {
    let Some(pools) = pools().await else { return };
    let app = app(&pools);
    let (_, _, token) = setup(&pools, &app).await;
    let bearer = format!("Bearer {token}");
    let scim = ("authorization", bearer.as_str());
    let stamp = unique("g").replace(' ', ".").to_lowercase();
    let mut ids = Vec::new();
    for n in 0..3 {
        ids.push(provision(&app, scim, &format!("{stamp}.{n}@contoso.example")).await);
    }
    let name = format!("Engineering {stamp}");
    let (status, group) = send(
        &app,
        "POST",
        "/scim/v2/Groups",
        scim,
        Some(
            json!({ "schemas": [GROUP], "displayName": name, "externalId": "grp-1",
                     "members": [{ "value": ids[0] }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let gid = group["id"].as_str().unwrap().to_owned();
    // It's a team.
    let team: String = sqlx::query_scalar("SELECT name FROM teams WHERE id = $1")
        .bind(Uuid::parse_str(&gid).unwrap())
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(team, name);

    // Entra adds and removes members with PATCH.
    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/scim/v2/Groups/{gid}"),
        scim,
        Some(json!({ "schemas": [PATCH], "Operations": [
            { "op": "Add", "path": "members", "value": [{ "value": ids[1] }, { "value": ids[2] }] },
            { "op": "Remove", "path": format!("members[value eq \"{}\"]", ids[0]) },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut expected = vec![ids[1].clone(), ids[2].clone()];
    expected.sort();
    assert_eq!(members(&app, scim, &gid).await, expected);

    // Lookups by name can leave members out.
    let (_, found) = send(
        &app,
        "GET",
        &format!(
            "/scim/v2/Groups?filter=displayName%20eq%20%22{}%22&excludedAttributes=members",
            name.replace(' ', "%20")
        ),
        scim,
        None,
    )
    .await;
    assert_eq!(found["totalResults"], 1);
    assert!(found["Resources"][0].get("members").is_none());

    // A deprovisioned user leaves the provider's teams.
    send(
        &app,
        "DELETE",
        &format!("/scim/v2/Users/{}", ids[1]),
        scim,
        None,
    )
    .await;
    assert_eq!(members(&app, scim, &gid).await.len(), 1);

    // Unknown members are refused; deleting the group removes the team.
    let (status, bad) = send(
        &app,
        "PATCH",
        &format!("/scim/v2/Groups/{gid}"),
        scim,
        Some(json!({ "schemas": [PATCH], "Operations": [
            { "op": "Add", "path": "members", "value": [{ "value": Uuid::now_v7() }] },
        ] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/scim/v2/Groups/{gid}"),
        scim,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let gone: bool = sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM teams WHERE id = $1)")
        .bind(Uuid::parse_str(&gid).unwrap())
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert!(gone);
}
