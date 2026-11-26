//! The client portal's realm through the API (ADR-040), on its own
//! database: staff invite a contact; the contact signs in with a link;
//! portal and staff sessions never cross.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "portal api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    staff: String,
    acme: Uuid,
    globex: Uuid,
}

async fn setup(pools: &TestPools) -> Setup {
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let owner = huxley_auth::create_user(&mut tx, &ctx, "staff@example.com", "Staff", PASSWORD)
        .await
        .unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, owner.id)
        .await
        .unwrap();
    for module in ["crm", "portal"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let client = async |tx: &mut sqlx::PgConnection, name: &str| {
        huxley_orgs::create(tx, &ctx, root, huxley_orgs::OrgKind::Client, name)
            .await
            .unwrap()
            .id
    };
    let acme = client(&mut tx, "Acme").await;
    let globex = client(&mut tx, "Globex").await;
    tx.commit().await.unwrap();
    let req = Request::post("/api/v1/auth/login")
        .header("x-requested-with", "huxley")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "email": "staff@example.com", "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let staff = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    Setup {
        pools: pools.clone(),
        app,
        staff,
        acme,
        globex,
    }
}

impl Setup {
    async fn call(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value, Option<String>) {
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
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let cookie = res
            .headers()
            .get("set-cookie")
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_owned());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            cookie,
        )
    }

    async fn contact(&self, org: Uuid, first: &str, email: &str) -> String {
        let (status, contact, _) = self
            .call(
                &self.staff,
                "POST",
                &format!("/api/v1/orgs/{org}/contacts"),
                json!({ "firstName": first, "lastName": "Client", "email": email }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{contact}");
        contact["id"].as_str().unwrap().to_owned()
    }
}

/// A portal session is nobody to the staff API, and a staff session is
/// nobody to the portal API.
async fn realms_dont_cross(s: &Setup, portal: &str) {
    let (status, _, _) = s.call(portal, "GET", "/api/v1/auth/me", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = s
        .call(
            portal,
            "GET",
            &format!("/api/v1/orgs/{}/contacts", s.acme),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = s
        .call(&s.staff, "GET", "/api/portal/v1/me", Value::Null)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A staff token shaped like a portal cookie doesn't work either.
    let staff_token = s.staff.split('=').nth(1).unwrap();
    let (status, _, _) = s
        .call(
            &format!("hx_portal_session={staff_token}"),
            "GET",
            "/api/portal/v1/me",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn contacts_sign_in_to_their_own_realm() {
    let Some(db) = isolated("portal_api").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let ada = s.contact(s.acme, "Ada", "ada@acme.example").await;

    // Staff invite Ada (no email set up here, so nothing is sent).
    let (status, invited, _) = s
        .call(
            &s.staff,
            "POST",
            &format!("/api/v1/contacts/{ada}/portal-access"),
            json!({ "role": "client_admin" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{invited}");
    assert_eq!(
        (&invited["account"]["status"], &invited["invitationSent"]),
        (&json!("invited"), &json!(false))
    );
    let account = invited["account"]["id"].as_str().unwrap().to_owned();
    let (_, listed, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/orgs/{}/portal-accounts", s.acme),
            Value::Null,
        )
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // Asking for a link answers the same for anyone.
    let (status, sent, _) = s
        .call(
            "",
            "POST",
            "/api/portal/v1/auth/link",
            json!({ "email": "nobody@example.com" }),
        )
        .await;
    assert_eq!(
        (status, &sent["sent"]),
        (StatusCode::ACCEPTED, &json!(true))
    );

    // Ada signs in with a link (taken from the database's side here).
    let secret = {
        let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
        let secret = huxley_portal::signin::issue_link(&mut tx, account.parse().unwrap())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        secret
    };
    let (status, me, cookie) = s
        .call(
            "",
            "POST",
            "/api/portal/v1/auth/redeem",
            json!({ "token": secret }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let portal = cookie.unwrap();
    assert!(portal.starts_with("hx_portal_session=hxps_"), "{portal}");
    assert_eq!(
        (&me["name"], &me["orgName"]),
        (&json!("Ada Client"), &json!("Acme"))
    );
    assert!(
        me["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("portal.contacts:manage"))
    );
    let (status, _, _) = s
        .call(&portal, "GET", "/api/portal/v1/me", Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);

    realms_dont_cross(&s, &portal).await;

    // Disabling ends the session; signing out clears the cookie.
    let (_, changed, _) = s
        .call(
            &s.staff,
            "PATCH",
            &format!("/api/v1/portal-accounts/{account}"),
            json!({ "role": "client_user", "status": "disabled" }),
        )
        .await;
    assert_eq!(changed["status"], "disabled");
    let (status, _, _) = s
        .call(&portal, "GET", "/api/portal/v1/me", Value::Null)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, cleared) = s
        .call(&portal, "POST", "/api/portal/v1/auth/logout", Value::Null)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(cleared.as_deref(), Some("hx_portal_session="));

    one_account_per_address(&s).await;
}

/// Another company's contact with the same address can't also have portal
/// access.
async fn one_account_per_address(s: &Setup) {
    let bob = s.contact(s.globex, "Bob", "ada@acme.example").await;
    let (status, _, _) = s
        .call(
            &s.staff,
            "POST",
            &format!("/api/v1/contacts/{bob}/portal-access"),
            json!({ "role": "client_user" }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "one portal account per address"
    );
}
