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
    for module in ["crm", "servicedesk", "portal", "knowledge", "cmdb"] {
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
