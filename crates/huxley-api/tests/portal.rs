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
    root: Uuid,
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
    for module in ["crm", "servicedesk", "portal", "knowledge", "cmdb", "cx"] {
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
        root,
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

impl Setup {
    /// Invites a contact with a role and signs them in: their portal cookie.
    async fn portal_user(&self, org: Uuid, first: &str, role: &str) -> (String, String) {
        let contact = self
            .contact(
                org,
                first,
                &format!("{}@{}.example", first.to_lowercase(), org.simple()),
            )
            .await;
        let (_, invited, _) = self
            .call(
                &self.staff,
                "POST",
                &format!("/api/v1/contacts/{contact}/portal-access"),
                json!({ "role": role }),
            )
            .await;
        let account: Uuid = invited["account"]["id"].as_str().unwrap().parse().unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let secret = huxley_portal::signin::issue_link(&mut tx, account)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let (_, _, cookie) = self
            .call(
                "",
                "POST",
                "/api/portal/v1/auth/redeem",
                json!({ "token": secret }),
            )
            .await;
        (cookie.unwrap(), contact)
    }
}

#[tokio::test]
async fn contacts_see_and_work_their_tickets_only() {
    let Some(db) = isolated("portal_tickets").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (bea, _) = s.portal_user(s.acme, "Bea", "client_admin").await;
    let (gil, _) = s.portal_user(s.globex, "Gil", "client_admin").await;

    // Ada raises a ticket; Bea raises one.
    let (status, raised, _) = s
        .call(
            &ada,
            "POST",
            "/api/portal/v1/tickets",
            json!({ "title": "Printer jammed", "description": "Floor 2" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{raised}");
    let ticket = raised["id"].as_str().unwrap().to_owned();
    assert_eq!(raised["requesterName"], "Ada Client");
    s.call(
        &bea,
        "POST",
        "/api/portal/v1/tickets",
        json!({ "title": "New starter" }),
    )
    .await;

    // Ada sees hers; she can't ask for the company's. Bea sees both.
    let (_, mine, _) = s
        .call(&ada, "GET", "/api/portal/v1/tickets", Value::Null)
        .await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    let (status, _, _) = s
        .call(
            &ada,
            "GET",
            "/api/portal/v1/tickets?scope=company",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, company, _) = s
        .call(
            &bea,
            "GET",
            "/api/portal/v1/tickets?scope=company",
            Value::Null,
        )
        .await;
    assert_eq!(company.as_array().unwrap().len(), 2);
    let bea_ticket = company
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "New starter")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Ada can't open Bea's; another company can't open either.
    let (status, _, _) = s
        .call(
            &ada,
            "GET",
            &format!("/api/portal/v1/tickets/{bea_ticket}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = s
        .call(
            &gil,
            "GET",
            &format!("/api/portal/v1/tickets/{ticket}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    internal_notes_stay_internal(&s, &ada, &ticket).await;
    resolved_tickets_reopen_on_reply(&s, &ada, &ticket).await;
}

/// The service desk answers publicly and notes internally: only the public
/// answer shows.
async fn internal_notes_stay_internal(s: &Setup, ada: &str, ticket: &str) {
    for (kind, body) in [("public", "On our way"), ("internal", "Toner order #42")] {
        let (status, _, _) = s
            .call(
                &s.staff,
                "POST",
                &format!("/api/v1/tickets/{ticket}/notes"),
                json!({ "kind": kind, "body": body }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (_, seen, _) = s
        .call(
            ada,
            "GET",
            &format!("/api/portal/v1/tickets/{ticket}"),
            Value::Null,
        )
        .await;
    let bodies: Vec<&str> = seen["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["On our way"]);
    assert_eq!(seen["notes"][0]["fromSupport"], true);
    assert!(!seen.to_string().contains("Toner order"));
}

/// A reply to a resolved ticket reopens it, and shows as the contact's.
async fn resolved_tickets_reopen_on_reply(s: &Setup, ada: &str, ticket: &str) {
    let resolved: String = sqlx::query_scalar(
        "SELECT s.key FROM sd_statuses s JOIN sd_tickets t ON t.type_id = s.type_id \
          WHERE t.id = $1 AND s.category = 'resolved' ORDER BY s.position LIMIT 1",
    )
    .bind(ticket.parse::<Uuid>().unwrap())
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    let (_, current, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/tickets/{ticket}"),
            Value::Null,
        )
        .await;
    let (status, done, _) = s
        .call(
            &s.staff,
            "POST",
            &format!("/api/v1/tickets/{ticket}/status"),
            json!({ "status": resolved, "resolution": "Cleared the jam", "version": current["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (_, closed, _) = s
        .call(
            ada,
            "GET",
            "/api/portal/v1/tickets?status=closed",
            Value::Null,
        )
        .await;
    assert_eq!(closed.as_array().unwrap().len(), 1);
    let (status, reply, _) = s
        .call(
            ada,
            "POST",
            &format!("/api/portal/v1/tickets/{ticket}/replies"),
            json!({ "body": "Jammed again" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    assert_eq!(
        (&reply["authorName"], &reply["fromSupport"]),
        (&json!("Ada Client"), &json!(false))
    );
    let (_, after, _) = s
        .call(
            ada,
            "GET",
            &format!("/api/portal/v1/tickets/{ticket}"),
            Value::Null,
        )
        .await;
    assert_eq!(after["statusCategory"], "open");
    let notes: Vec<bool> = after["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["resolution"].as_bool().unwrap())
        .collect();
    assert_eq!(notes, [false, true, false]);
}

#[tokio::test]
async fn the_portal_wears_the_msps_brand_and_a_clients_own() {
    let Some(db) = isolated("portal_branding").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let set = async |org: Uuid, key: &str, value: &str| {
        s.call(
            &s.staff,
            "PUT",
            &format!("/api/v1/orgs/{org}/settings/{key}"),
            json!({ "value": value }),
        )
        .await
        .0
    };
    assert_eq!(
        set(s.root, "branding.name", "Contoso Support").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set(s.root, "branding.accent_color", "#112233").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set(s.root, "portal.domain", "Support.Contoso.test").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set(
            s.root,
            "branding.logo",
            "data:image/png;base64,iVBORw0KGgo="
        )
        .await,
        StatusCode::NO_CONTENT
    );
    // Acme gets its own accent; a script isn't a logo; a domain is one organization's.
    assert_eq!(
        set(s.acme, "branding.accent_color", "#aa0000").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        set(
            s.acme,
            "branding.logo",
            "data:text/html;base64,PHNjcmlwdD4="
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        set(s.globex, "portal.domain", "support.contoso.test").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Before signing in: by the portal's host.
    let (status, brand, _) = s
        .call(
            "",
            "GET",
            "/api/portal/v1/branding?host=support.contoso.test",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&brand["name"], &brand["accentColor"]),
        (&json!("Contoso Support"), &json!("#112233"))
    );
    assert_eq!(brand["logo"], "data:image/png;base64,iVBORw0KGgo=");
    let (_, unknown, _) = s
        .call(
            "",
            "GET",
            "/api/portal/v1/branding?host=elsewhere.test",
            Value::Null,
        )
        .await;
    assert_eq!(
        unknown["name"], "Contoso Support",
        "unknown hosts get the MSP's"
    );

    // Signed in: their company's (Acme's accent, the MSP's name and logo).
    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (_, me, _) = s.call(&ada, "GET", "/api/portal/v1/me", Value::Null).await;
    assert_eq!(
        (&me["branding"]["name"], &me["branding"]["accentColor"]),
        (&json!("Contoso Support"), &json!("#aa0000"))
    );
}

#[tokio::test]
async fn identity_providers_are_kept_by_staff_and_only_return_to_this_portal() {
    let Some(db) = isolated("portal_idp").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let (status, provider, _) = s
        .call(
            &s.staff,
            "POST",
            &format!("/api/v1/orgs/{}/portal-identity-providers", s.acme),
            json!({
                "name": "Acme (Microsoft)", "issuerUrl": "https://login.microsoftonline.com/acme-tenant/v2.0",
                "clientId": "app-1", "clientSecret": "shh", "emailDomains": ["acme.example"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{provider}");
    assert_eq!(provider["hasSecret"], true);
    assert!(
        !provider.to_string().contains("shh"),
        "the secret is write-only"
    );

    // No provider for a domain: use a link.
    let start = |email: &str, redirect: &str| json!({ "email": email, "redirect": redirect });
    let (status, _, _) = s
        .call(
            "",
            "POST",
            "/api/portal/v1/auth/sso/start",
            start("ada@elsewhere.example", "x"),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Codes only ever come back to this portal's callback page.
    let (status, _, _) = s
        .call(
            "",
            "POST",
            "/api/portal/v1/auth/sso/start",
            start("ada@acme.example", "https://evil.example/auth/sso/callback"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn contacts_ask_for_things_with_portal_forms() {
    let Some(db) = isolated("portal_catalog").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let form = |key: &str, portal: bool, org: Uuid| {
        json!({ "orgId": org, "form": {
            "key": key, "name": format!("Form {key}"), "portal": portal, "portalCategory": "Accounts",
            "schema": { "type": "object", "required": ["who"], "properties": { "who": { "type": "string" } } },
        }})
    };
    // The MSP offers one to every client; Globex offers its own; one isn't offered.
    for (key, portal, org) in [
        ("new-user", true, s.root),
        ("globex-only", true, s.globex),
        ("internal", false, s.root),
    ] {
        let (status, body, _) = s
            .call(&s.staff, "POST", "/api/v1/forms", form(key, portal, org))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    // A form that looks choices up through an integration can't be offered.
    let mut lookup = form("lookup", true, s.root);
    lookup["form"]["ui"] =
        json!({ "who": { "options_from": { "integration": "x", "action": "y" } } });
    let (status, _, _) = s.call(&s.staff, "POST", "/api/v1/forms", lookup).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (_, catalog, _) = s
        .call(&ada, "GET", "/api/portal/v1/catalog", Value::Null)
        .await;
    let names: Vec<&str> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Form new-user"]);
    assert_eq!(catalog[0]["category"], "Accounts");
    let id = catalog[0]["id"].as_str().unwrap();

    let uri = format!("/api/portal/v1/catalog/{id}/requests");
    let (status, _, _) = s.call(&ada, "POST", &uri, json!({ "answers": {} })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, created, _) = s
        .call(&ada, "POST", &uri, json!({ "answers": { "who": "Bob" } }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, requests, _) = s
        .call(&ada, "GET", "/api/portal/v1/requests", Value::Null)
        .await;
    assert_eq!(requests.as_array().unwrap().len(), 1, "{requests}");
    assert_eq!(
        (&requests[0]["formName"], &requests[0]["status"]),
        (&json!("Form new-user"), &json!("received"))
    );
    // Globex's own form isn't Acme's to use.
    let (_, forms, _) = s.call(&s.staff, "GET", "/api/v1/forms", Value::Null).await;
    let globex_form = forms
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "globex-only")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _, _) = s
        .call(
            &ada,
            "POST",
            &format!("/api/portal/v1/catalog/{globex_form}/requests"),
            json!({ "answers": { "who": "x" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn contacts_read_published_articles_meant_for_clients() {
    let Some(db) = isolated("portal_kb").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let article = async |org: Uuid, title: &str, visibility: &str, publish: bool| {
        let (status, a, _) = s
            .call(
                &s.staff,
                "POST",
                "/api/v1/kb/articles",
                json!({ "orgId": org, "title": title, "body": format!("How to **{title}**: reset it."), "visibility": visibility }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{a}");
        let id = a["id"].as_str().unwrap().to_owned();
        if publish {
            let (status, p, _) = s
                .call(
                    &s.staff,
                    "POST",
                    &format!("/api/v1/kb/articles/{id}/publish"),
                    json!({}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{p}");
        }
        id
    };
    let shared = article(s.root, "Reset your password", "clients", true).await;
    article(s.root, "Internal runbook password", "internal", true).await;
    article(s.root, "Draft password guide", "clients", false).await;
    let globex = article(s.globex, "Globex VPN password", "public", true).await;

    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (_, me, _) = s.call(&ada, "GET", "/api/portal/v1/me", Value::Null).await;
    assert!(
        me["modules"]
            .as_array()
            .unwrap()
            .contains(&json!("knowledge")),
        "{me}"
    );
    let (status, list, _) = s
        .call(
            &ada,
            "GET",
            "/api/portal/v1/articles?q=password",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let titles: Vec<&str> = list["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Reset your password"]);
    assert!(
        list["articles"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("<b>")
    );
    let (_, one, _) = s
        .call(
            &ada,
            "GET",
            &format!("/api/portal/v1/articles/{shared}"),
            Value::Null,
        )
        .await;
    assert!(
        one["bodyHtml"]
            .as_str()
            .unwrap()
            .contains("<strong>Reset your password</strong>"),
        "{one}"
    );
    let (status, _, _) = s
        .call(
            &ada,
            "GET",
            &format!("/api/portal/v1/articles/{globex}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn contacts_see_their_devices_and_raise_tickets_about_them() {
    let Some(db) = isolated("portal_devices").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let (ada, ada_contact) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (bea, _) = s.portal_user(s.acme, "Bea", "client_admin").await;
    let (_, types, _) = s
        .call(&s.staff, "GET", "/api/v1/asset-types", Value::Null)
        .await;
    let workstation = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "workstation")
        .unwrap()["id"]
        .clone();
    let asset = async |org: Uuid, name: &str, contact: Option<&str>, status: &str| {
        let (code, a, _) = s
            .call(
                &s.staff,
                "POST",
                "/api/v1/assets",
                json!({ "orgId": org, "typeId": workstation, "name": name, "contactId": contact,
                        "status": status, "notes": "Admin password in the vault", "serialNumber": format!("SN-{name}") }),
            )
            .await;
        assert_eq!(code, StatusCode::CREATED, "{a}");
        a["id"].as_str().unwrap().to_owned()
    };
    let adas = asset(s.acme, "ADA-LAPTOP", Some(&ada_contact), "active").await;
    asset(s.acme, "FRONT-DESK", None, "active").await;
    asset(s.acme, "OLD-PC", None, "retired").await;
    let globex = asset(s.globex, "GLOBEX-PC", None, "active").await;

    let names = async |cookie: &str| -> Vec<String> {
        let (_, list, _) = s
            .call(cookie, "GET", "/api/portal/v1/devices", Value::Null)
            .await;
        list.as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_owned())
            .collect()
    };
    // Ada sees her own; Bea (client admin) sees the company's in use.
    assert_eq!(names(&ada).await, ["ADA-LAPTOP"]);
    assert_eq!(names(&bea).await, ["ADA-LAPTOP", "FRONT-DESK"]);
    let (_, device, _) = s
        .call(
            &ada,
            "GET",
            &format!("/api/portal/v1/devices/{adas}"),
            Value::Null,
        )
        .await;
    assert_eq!(device["serialNumber"], "SN-ADA-LAPTOP");
    assert!(
        device.get("notes").is_none(),
        "staff notes stay internal: {device}"
    );
    let (status, _, _) = s
        .call(
            &bea,
            "GET",
            &format!("/api/portal/v1/devices/{globex}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A ticket about her laptop is linked to it; another client's device isn't hers to name.
    let (status, ticket, _) = s
        .call(
            &ada,
            "POST",
            "/api/portal/v1/tickets",
            json!({ "title": "Laptop is slow", "assetId": adas }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ticket}");
    let (_, linked, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/tickets/{}/assets", ticket["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
    assert_eq!(linked["linked"][0]["name"], "ADA-LAPTOP", "{linked}");
    let (status, _, _) = s
        .call(
            &ada,
            "POST",
            "/api/portal/v1/tickets",
            json!({ "title": "Not mine", "assetId": globex }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn client_admins_see_published_dashboards_with_their_numbers() {
    let Some(db) = isolated("portal_dashboards").await else {
        return;
    };
    let s = setup(&db.pools).await;
    for (org, n) in [(s.acme, 2), (s.globex, 5)] {
        for i in 0..n {
            let (status, t, _) = s
                .call(
                    &s.staff,
                    "POST",
                    "/api/v1/tickets",
                    json!({ "orgId": org, "title": format!("T{i}") }),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{t}");
        }
    }
    let count = json!([{ "fn": "count" }]);
    let board = |portal: bool, name: &str| {
        json!({ "orgId": s.root, "shared": true, "dashboard": {
        "name": name, "portal": portal, "widgets": [
            { "id": "open", "title": "Tickets", "viz": "number", "width": 6, "spec": { "source": "tickets", "measures": count } },
            { "id": "runs", "title": "Runs", "viz": "number", "width": 6, "spec": { "source": "runs", "measures": count } },
        ] } })
    };
    let (status, published, _) = s
        .call(
            &s.staff,
            "POST",
            "/api/v1/dashboards",
            board(true, "Your service"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    s.call(
        &s.staff,
        "POST",
        "/api/v1/dashboards",
        board(false, "Internal"),
    )
    .await;
    // Only shared dashboards go to the portal.
    let mut personal = board(true, "Mine");
    personal["shared"] = json!(false);
    assert_eq!(
        s.call(&s.staff, "POST", "/api/v1/dashboards", personal)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let (bea, _) = s.portal_user(s.acme, "Bea", "client_admin").await;
    let (_, list, _) = s
        .call(&bea, "GET", "/api/portal/v1/dashboards", Value::Null)
        .await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Your service"]);
    let id = published["id"].as_str().unwrap();
    let (status, view, _) = s
        .call(
            &bea,
            "GET",
            &format!("/api/portal/v1/dashboards/{id}?range=this_year"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["range"], "this_year");
    assert_eq!(
        view["results"]["open"]["answer"]["rows"],
        json!([[2]]),
        "Acme's two, not Globex's: {view}"
    );
    assert_eq!(view["results"]["runs"]["problem"], "Not available here.");

    // Client users don't see dashboards.
    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    assert_eq!(
        s.call(&ada, "GET", "/api/portal/v1/dashboards", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn client_admins_manage_their_colleagues_access() {
    let Some(db) = isolated("portal_people").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let (bea, bea_contact) = s.portal_user(s.acme, "Bea", "client_admin").await;
    let cy = s.contact(s.acme, "Cy", "cy@acme.example").await;
    let gil = s.contact(s.globex, "Gil", "gil@globex.example").await;

    let (_, people, _) = s
        .call(&bea, "GET", "/api/portal/v1/people", Value::Null)
        .await;
    let names: Vec<&str> = people
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Bea Client", "Cy Client"]);
    assert_eq!(people[1]["accountId"], Value::Null);

    // Add a colleague with access; give Cy access; Globex's people aren't hers.
    let (status, dee, _) = s
        .call(&bea, "POST", "/api/portal/v1/people", json!({ "firstName": "Dee", "lastName": "New", "email": "dee@acme.example", "role": "client_user" }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{dee}");
    let (status, _, _) = s
        .call(
            &bea,
            "POST",
            "/api/portal/v1/people",
            json!({ "firstName": "Dee", "email": "DEE@acme.example" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let access = |c: &str| format!("/api/portal/v1/people/{c}/access");
    let (status, _, _) = s
        .call(
            &bea,
            "POST",
            &access(&cy),
            json!({ "role": "client_approver" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        s.call(
            &bea,
            "POST",
            &access(&gil),
            json!({ "role": "client_user" })
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (_, people, _) = s
        .call(&bea, "GET", "/api/portal/v1/people", Value::Null)
        .await;
    let cy_row = people
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Cy Client")
        .unwrap()
        .clone();
    assert_eq!(
        (&cy_row["role"], &cy_row["status"]),
        (&json!("client_approver"), &json!("invited"))
    );
    let dee_row = people
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Dee New")
        .unwrap()
        .clone();
    assert_eq!(dee_row["role"], "client_user");

    changes_and_limits(&s, &bea, &bea_contact, &cy, &cy_row, &dee).await;
}

/// A client admin changes a colleague's role, ends another's access, never
/// changes her own; client users don't manage people.
async fn changes_and_limits(
    s: &Setup,
    bea: &str,
    bea_contact: &str,
    cy: &str,
    cy_row: &Value,
    dee: &Value,
) {
    let access = |c: &str| format!("/api/portal/v1/people/{c}/access");
    // Change Cy's role, then end Dee's access; never her own.
    let (status, changed, _) = s
        .call(
            bea,
            "PATCH",
            &access(cy),
            json!({ "role": "client_admin", "status": "invited", "version": cy_row["version"] }),
        )
        .await;
    assert_eq!(
        (status, &changed["role"]),
        (StatusCode::OK, &json!("client_admin")),
        "{changed}"
    );
    let dee_contact = dee["contactId"].as_str().unwrap();
    let (_, ended, _) = s
        .call(
            bea,
            "PATCH",
            &access(dee_contact),
            json!({ "role": "client_user", "status": "disabled" }),
        )
        .await;
    assert_eq!(ended["status"], "disabled");
    let (status, _, _) = s
        .call(
            bea,
            "PATCH",
            &access(bea_contact),
            json!({ "role": "client_user", "status": "active" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Client users don't manage people.
    let (ada, _) = s.portal_user(s.acme, "Ada", "client_user").await;
    assert_eq!(
        s.call(&ada, "GET", "/api/portal/v1/people", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

impl Setup {
    /// A resolved ticket for `requester` at Acme: its id.
    async fn resolved_ticket(&self, requester: &str, title: &str) -> Uuid {
        let (_, t, _) = self
            .call(
                &self.staff,
                "POST",
                "/api/v1/tickets",
                json!({ "orgId": self.acme, "title": title, "requesterId": requester }),
            )
            .await;
        let id: Uuid = t["id"].as_str().unwrap().parse().unwrap();
        let resolved: String = sqlx::query_scalar(
            "SELECT s.key FROM sd_statuses s JOIN sd_tickets t ON t.type_id = s.type_id \
              WHERE t.id = $1 AND s.category = 'resolved' ORDER BY s.position LIMIT 1",
        )
        .bind(id)
        .fetch_one(&self.pools.owner)
        .await
        .unwrap();
        let (status, done, _) = self
            .call(
                &self.staff,
                "POST",
                &format!("/api/v1/tickets/{id}/status"),
                json!({ "status": resolved, "resolution": "Done", "version": t["version"] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{done}");
        id
    }

    /// Starts a ticket's survey as the pack's step would (sampling passes).
    async fn survey(&self, ticket: Uuid) -> huxley_cx::surveys::Started {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let started = huxley_cx::surveys::start(&mut tx, &ctx, ticket, 0.0)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        started
    }
}

#[tokio::test]
async fn requesters_rate_resolved_tickets() {
    let Some(db) = isolated("portal_csat").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let (ada, ada_contact) = s.portal_user(s.acme, "Ada", "client_user").await;
    let ticket = s.resolved_ticket(&ada_contact, "Printer jam").await;
    assert!(matches!(
        s.survey(ticket).await,
        huxley_cx::surveys::Started::Survey { .. }
    ));
    // Asked once a week at most.
    assert!(matches!(
        s.survey(ticket).await,
        huxley_cx::surveys::Started::Skipped("already_asked")
    ));

    // Ada rates it in the portal, once.
    let (_, view, _) = s
        .call(
            &ada,
            "GET",
            &format!("/api/portal/v1/tickets/{ticket}"),
            Value::Null,
        )
        .await;
    assert_eq!(
        (&view["survey"]["scale"], &view["survey"]["open"]),
        (&json!("five"), &json!(true)),
        "{view}"
    );
    let rate = format!("/api/portal/v1/tickets/{ticket}/rating");
    assert_eq!(
        s.call(&ada, "POST", &rate, json!({ "rating": 9 })).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, rated, _) = s
        .call(
            &ada,
            "POST",
            &rate,
            json!({ "rating": 4, "comment": "Quick, thanks" }),
        )
        .await;
    assert_eq!(
        (status, &rated["survey"]["rating"]),
        (StatusCode::OK, &json!(4)),
        "{rated}"
    );
    assert_eq!(
        s.call(&ada, "POST", &rate, json!({ "rating": 1 })).await.0,
        StatusCode::CONFLICT
    );
    let (_, staff_view, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/tickets/{ticket}/csat"),
            Value::Null,
        )
        .await;
    assert_eq!(
        (&staff_view["comment"], &staff_view["channel"]),
        (&json!("Quick, thanks"), &json!("portal"))
    );

    answers_by_link_and_settings(&s, &ada_contact).await;
}

/// The emailed link answers without signing in; thumbs and opting out and
/// sampling follow the company's settings.
async fn answers_by_link_and_settings(s: &Setup, ada_contact: &str) {
    let ctx = RequestContext::system(Via::System);
    let set = async |key: &str, value: &str| {
        let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
        huxley_orgs::settings::set(&mut tx, &ctx, s.acme, key, value, false)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    };
    set("csat.thumbs", "true").await;
    let ticket = s.resolved_ticket(ada_contact, "VPN down").await;
    let huxley_cx::surveys::Started::Survey { token, .. } = s.survey(ticket).await else {
        panic!("not sent")
    };
    let link = format!("/api/portal/v1/surveys/{token}");
    let (status, shown, _) = s.call("", "GET", &link, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(
        (
            &shown["scale"],
            &shown["open"],
            &shown["reference"].is_string()
        ),
        (&json!("thumbs"), &json!(true), &true)
    );
    assert_eq!(
        s.call("", "POST", &link, json!({ "rating": 3 })).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, answered, _) = s
        .call(
            "",
            "POST",
            &link,
            json!({ "rating": 1, "comment": "Still down" }),
        )
        .await;
    assert_eq!(
        (status, &answered["open"]),
        (StatusCode::OK, &json!(false)),
        "{answered}"
    );
    assert_eq!(
        s.call("", "GET", "/api/portal/v1/surveys/hxcs_madeup", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    set("csat.sample_percent", "0").await;
    let sampled = s.resolved_ticket(ada_contact, "Sampled").await;
    assert!(matches!(
        s.survey(sampled).await,
        huxley_cx::surveys::Started::Skipped("sampled_out")
    ));
    set("csat.enabled", "false").await;
    let opted = s.resolved_ticket(ada_contact, "Opted out").await;
    assert!(matches!(
        s.survey(opted).await,
        huxley_cx::surveys::Started::Skipped("opted_out")
    ));

    // A satisfaction score: one up (4/5), one down.
    let (status, score, _) = s
        .call(
            &s.staff,
            "POST",
            "/api/v1/analytics/query",
            json!({ "source": "csat",
            "measures": [{ "fn": "avg", "field": "satisfaction" }, { "fn": "count" }],
            "filters": [{ "field": "answered", "op": "eq", "value": true }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{score}");
    assert_eq!(score["rows"], json!([[50.0, 2]]), "{score}");
}

#[tokio::test]
async fn contacts_answer_the_net_promoter_question() {
    let Some(db) = isolated("portal_nps").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let ctx = RequestContext::system(Via::System);
    let (ada, ada_contact) = s.portal_user(s.acme, "Ada", "client_user").await;
    let bea = s.contact(s.acme, "Bea", "bea@acme.example").await;
    s.contact(s.acme, "Cal", "cal@acme.example").await; // raised nothing: not asked
    s.resolved_ticket(&ada_contact, "Printer").await;
    s.resolved_ticket(&bea, "VPN").await;

    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let who = huxley_cx::nps::candidates(&mut tx, s.root, 100)
        .await
        .unwrap();
    let mut names: Vec<Uuid> = who.iter().map(|c| c.contact_id).collect();
    names.sort();
    let mut expected: Vec<Uuid> = [&ada_contact, &bea]
        .iter()
        .map(|c| c.parse().unwrap())
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    let mut tokens = std::collections::HashMap::new();
    for c in &who {
        let (_, token) = huxley_cx::nps::start(&mut tx, &ctx, c, "2026-11")
            .await
            .unwrap();
        tokens.insert(c.contact_id, token);
    }
    // Asked lately: not again.
    assert!(
        huxley_cx::nps::candidates(&mut tx, s.root, 100)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();

    // Ada answers in the portal.
    let (_, mine, _) = s
        .call(&ada, "GET", "/api/portal/v1/me/nps", Value::Null)
        .await;
    assert_eq!(mine["open"], true, "{mine}");
    assert_eq!(
        s.call(
            &ada,
            "POST",
            "/api/portal/v1/me/nps",
            json!({ "score": 11 })
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, answered, _) = s
        .call(
            &ada,
            "POST",
            "/api/portal/v1/me/nps",
            json!({ "score": 3, "comment": "Slow" }),
        )
        .await;
    assert_eq!(
        (status, &answered["score"]),
        (StatusCode::OK, &json!(3)),
        "{answered}"
    );
    let (_, none, _) = s
        .call(&ada, "GET", "/api/portal/v1/me/nps", Value::Null)
        .await;
    assert_eq!(none, Value::Null);

    // Bea answers from the email's link, once.
    let link = format!(
        "/api/portal/v1/nps/{}",
        tokens[&bea.parse::<Uuid>().unwrap()]
    );
    let (_, shown, _) = s.call("", "GET", &link, Value::Null).await;
    assert_eq!(shown["open"], true, "{shown}");
    assert_eq!(
        s.call("", "POST", &link, json!({ "score": 10 })).await.0,
        StatusCode::OK
    );
    assert_eq!(
        s.call("", "POST", &link, json!({ "score": 0 })).await.0,
        StatusCode::CONFLICT
    );
    let (_, staff_view, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/contacts/{bea}/nps"),
            Value::Null,
        )
        .await;
    assert_eq!(
        (&staff_view["score"], &staff_view["channel"]),
        (&json!(10), &json!("email"))
    );

    nps_score_and_opt_out(&s).await;
}

/// One detractor and one promoter make an NPS of 0; a company that opts out
/// isn't asked.
async fn nps_score_and_opt_out(s: &Setup) {
    let ctx = RequestContext::system(Via::System);
    // One detractor and one promoter: an NPS of 0.
    let (_, score, _) = s
        .call(&s.staff, "POST", "/api/v1/analytics/query", json!({ "source": "nps",
            "measures": [{ "fn": "avg", "field": "nps" }], "filters": [{ "field": "answered", "op": "eq", "value": true }] }))
        .await;
    assert_eq!(score["rows"], json!([[0.0]]), "{score}");
    // A company that opts out isn't asked.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_orgs::settings::set(&mut tx, &ctx, s.globex, "nps.enabled", "false", false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    s.contact(s.globex, "Gil", "gil@globex.example").await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    assert!(
        huxley_cx::nps::candidates(&mut tx, s.globex, 100)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn client_health_is_scored_with_reasons_and_declines_are_flagged() {
    let Some(db) = isolated("client_health").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let ctx = RequestContext::system(Via::System);
    let (_, ada) = s.portal_user(s.acme, "Ada", "client_user").await;
    // Six tickets; three missed an SLA target. Four happy survey answers.
    let mut tickets = Vec::new();
    for i in 0..6 {
        tickets.push(s.resolved_ticket(&ada, &format!("T{i}")).await);
    }
    for (i, t) in tickets.iter().enumerate() {
        sqlx::query(
            "INSERT INTO sd_ticket_sla (ticket_id, org_id, policy_name, calendar, targets, resolution_breached_at) \
             VALUES ($1, $2, 'Test', '{\"kind\": \"always\"}', '{}', CASE WHEN $3 THEN now() END) \
             ON CONFLICT (ticket_id) DO UPDATE SET resolution_breached_at = CASE WHEN $3 THEN now() END",
        )
        .bind(t)
        .bind(s.acme)
        .bind(i < 3)
        .execute(&s.pools.owner)
        .await
        .unwrap();
    }
    for t in &tickets[..4] {
        let huxley_cx::surveys::Started::Survey { survey, .. } = s.survey(*t).await else {
            panic!("not sent")
        };
        let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
        huxley_cx::surveys::respond(&mut tx, &ctx, &survey, 5, None, "portal")
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let today = time::OffsetDateTime::now_utc().date();
    // A week ago it was doing fine.
    sqlx::query("INSERT INTO cx_health (org_id, computed_on, score, factors, reasons) VALUES ($1, $2, 95, '[]', '[]')")
        .bind(s.acme)
        .bind(today - time::Duration::days(7))
        .execute(&s.pools.owner)
        .await
        .unwrap();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let huxley_cx::health::Computed {
        companies: scored,
        declining,
        ..
    } = huxley_cx::health::compute(&mut tx, &ctx, s.root, today)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(scored >= 2, "{scored}");
    // CSAT 100 (weight 3), SLA 0 (half missed, weight 3), none reopened 100 (weight 2): 63.
    assert_eq!(declining.len(), 1, "{declining:?}");
    assert_eq!(
        (
            declining[0].org_id,
            declining[0].score,
            declining[0].previous
        ),
        (s.acme, 63, 95)
    );
    assert!(
        declining[0].reasons[0].starts_with("SLA targets met: 50% of 6 tickets"),
        "{:?}",
        declining[0].reasons
    );

    let (_, health, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/orgs/{}/health", s.acme),
            Value::Null,
        )
        .await;
    assert_eq!(health["score"], 63, "{health}");
    assert_eq!(health["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        health["computedOn"],
        json!(format!(
            "{:04}-{:02}-{:02}",
            today.year(),
            u8::from(today.month()),
            today.day()
        ))
    );
    health_model(&s, today).await;
}

/// Weights change the score; unknown factors are refused.
async fn health_model(s: &Setup, today: time::Date) {
    let ctx = RequestContext::system(Via::System);
    let model = format!("/api/v1/orgs/{}/health-model", s.root);
    let (_, current, _) = s.call(&s.staff, "GET", &model, Value::Null).await;
    assert_eq!(current["weights"]["sla"], 3);
    let (status, _, _) = s
        .call(&s.staff, "PUT", &model, json!({ "weights": { "luck": 5 } }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, set, _) = s
        .call(&s.staff, "PUT", &model, json!({ "weights": { "sla": 0 } }))
        .await;
    assert_eq!(
        (status, &set["weights"]["sla"]),
        (StatusCode::OK, &json!(0)),
        "{set}"
    );
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_cx::health::compute(&mut tx, &ctx, s.root, today)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (_, health, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/orgs/{}/health", s.acme),
            Value::Null,
        )
        .await;
    assert_eq!(health["score"], 100, "{health}");
}

#[tokio::test]
async fn falling_sentiment_flags_a_client_with_explanations() {
    let Some(db) = isolated("sentiment").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let ctx = RequestContext::system(Via::System);
    let (_, ada) = s.portal_user(s.acme, "Ada", "client_user").await;
    let mut tickets = Vec::new();
    for i in 0..6 {
        tickets.push(s.resolved_ticket(&ada, &format!("S{i}")).await);
    }
    let judged = [
        (1, "Thankful for the quick fix"),
        (1, "Pleased with the help"),
        (0, "A plain request"),
        (-2, "Third outage this week; threatening to leave"),
        (-1, "Frustrated by slow replies"),
        (-1, "Disappointed the printer broke again"),
    ];
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    assert!(
        huxley_cx::sentiment::record(&mut tx, tickets[0], None, 3, "Too happy")
            .await
            .is_err()
    );
    for (t, (score, reason)) in tickets.iter().zip(judged) {
        huxley_cx::sentiment::record(&mut tx, *t, None, score, reason)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    // The first three were 45 days ago: the client was fine then.
    sqlx::query(
        "UPDATE cx_sentiment SET at = now() - interval '45 days' WHERE ticket_id = ANY($1)",
    )
    .bind(&tickets[..3])
    .execute(&s.pools.owner)
    .await
    .unwrap();

    let today = time::OffsetDateTime::now_utc().date();
    let compute = || async {
        let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
        let c = huxley_cx::health::compute(&mut tx, &ctx, s.root, today)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        c
    };
    let first = compute().await;
    let acme: Vec<_> = first
        .sentiment_falling
        .iter()
        .filter(|f| f.org_id == s.acme)
        .collect();
    assert_eq!(acme.len(), 1, "{first:?}");
    assert!(
        (acme[0].recent + 4.0 / 3.0).abs() < 0.01 && (acme[0].previous - 2.0 / 3.0).abs() < 0.01
    );
    assert!(
        acme[0].explanations[0].ends_with(": Third outage this week; threatening to leave"),
        "{:?}",
        acme[0].explanations
    );
    assert_eq!(acme[0].explanations.len(), 3);
    // Flagged once a week, however often it's computed.
    assert!(
        compute()
            .await
            .sentiment_falling
            .iter()
            .all(|f| f.org_id != s.acme)
    );
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'client.sentiment_declining' AND subject_id = $1",
    )
    .bind(s.acme)
    .fetch_all(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (&events[0]["recent"], &events[0]["previous"]),
        (&json!(-1.3), &json!(0.7)),
        "{}",
        events[0]
    );

    sentiment_shows(&s, tickets[3]).await;
}

/// The health card shows sentiment as a factor, and the ticket its judgment.
async fn sentiment_shows(s: &Setup, ticket: Uuid) {
    let (_, health, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/orgs/{}/health", s.acme),
            Value::Null,
        )
        .await;
    let factor = health["factors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["key"] == "sentiment")
        .unwrap()
        .clone();
    assert_eq!(factor["score"], json!(17.0), "{factor}");
    assert!(
        factor["detail"].as_str().unwrap().contains("(e.g. "),
        "{factor}"
    );
    let (_, judged, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/tickets/{ticket}/sentiment"),
            Value::Null,
        )
        .await;
    assert_eq!(
        (&judged[0]["score"], judged.as_array().unwrap().len()),
        (&json!(-2), 1),
        "{judged}"
    );
}

const DOT_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

impl Setup {
    /// A quote for `org`, sent: its id.
    async fn sent_quote(&self, org: Uuid, title: &str, contact: Option<&str>) -> String {
        let (status, q, _) = self
            .call(
                &self.staff,
                "POST",
                "/api/v1/quotes",
                json!({ "orgId": org, "quote": { "title": title, "contactId": contact, "lines": [
                    { "name": "Laptop", "quantity": 2, "unitCost": "800", "unitPrice": "1000" },
                    { "name": "Dock", "optional": true, "unitCost": "120", "unitPrice": "200" }
                ] } }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{q}");
        let id = q["id"].as_str().unwrap().to_owned();
        let (status, _, _) = self
            .call(
                &self.staff,
                "POST",
                &format!("/api/v1/quotes/{id}/send"),
                json!({ "version": q["version"] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        id
    }
}

#[tokio::test]
async fn clients_sign_quotes_in_the_portal() {
    let Some(db) = isolated("portal_quotes").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    for module in ["contracts", "sales"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let (ada, ada_contact) = s.portal_user(s.acme, "Ada", "client_user").await;
    let (bob, _) = s.portal_user(s.acme, "Bob", "client_approver").await;
    let hers = s
        .sent_quote(s.acme, "Laptops for Ada", Some(&ada_contact))
        .await;
    let company = s.sent_quote(s.acme, "Printers", None).await;
    let other = s.sent_quote(s.globex, "Not yours", None).await;

    // Ada sees what's addressed to her, without costs; Bob sees the company's.
    let (_, mine, _) = s
        .call(&ada, "GET", "/api/portal/v1/quotes", Value::Null)
        .await;
    let titles: Vec<&str> = mine
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Laptops for Ada"]);
    assert!(mine[0]["lines"][0].get("unitCost").is_none());
    assert!(mine[0]["totals"].get("marginPct").is_none());
    let (_, all, _) = s
        .call(&bob, "GET", "/api/portal/v1/quotes", Value::Null)
        .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    for (who, id) in [(&ada, &company), (&bob, &other)] {
        let (status, _, _) = s
            .call(
                who,
                "GET",
                &format!("/api/portal/v1/quotes/{id}"),
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // Accepting needs agreement and a drawn signature.
    let answer = format!("/api/portal/v1/quotes/{hers}/answer");
    let dock = mine[0]["lines"][1]["id"].clone();
    let base = json!({ "accept": true, "name": "Ada Client", "options": [dock], "revision": 1 });
    let mut no_sig = base.clone();
    no_sig["agree"] = json!(true);
    let (status, problem, _) = s.call(&ada, "POST", &answer, no_sig).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let mut signed = base.clone();
    signed["signature"] = json!(DOT_PNG);
    let (status, _, _) = s.call(&ada, "POST", &answer, signed.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    signed["agree"] = json!(true);
    let (status, accepted, _) = s.call(&ada, "POST", &answer, signed.clone()).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["totals"]["oneTime"], "2200");
    let (status, _, _) = s.call(&ada, "POST", &answer, signed).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Staff see who signed, what, and the drawing.
    let (status, sig, _) = s
        .call(
            &s.staff,
            "GET",
            &format!("/api/v1/quotes/{hers}/signature"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sig}");
    assert_eq!(sig["name"], "Ada Client");
    assert_eq!(sig["contactName"], "Ada Client");
    assert_eq!(sig["hasImage"], true);
    assert_eq!(sig["documentSha256"].as_str().unwrap().len(), 64);
    let req = Request::get(format!("/api/v1/quotes/{hers}/signature.png"))
        .header("x-requested-with", "huxley")
        .header("cookie", &s.staff)
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.headers()["content-type"], "image/png");

    // Bob declines the company's quote, saying why.
    let (status, declined, _) = s
        .call(
            &bob,
            "POST",
            &format!("/api/portal/v1/quotes/{company}/answer"),
            json!({ "accept": false, "name": "Bob Client", "reason": "Not this year", "revision": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{declined}");
    assert_eq!(declined["status"], "declined");
}
