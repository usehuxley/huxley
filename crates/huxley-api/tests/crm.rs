//! The `crm` module's API against a real PostgreSQL: company details,
//! sites, contacts and their companies, custom fields, permissions and the
//! module gate.

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

const PASSWORD: &str = "crm api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    acme: Uuid,
    globex: Uuid,
    owner: String,
}

async fn org(pools: &TestPools, parent: Uuid, kind: huxley_orgs::OrgKind, name: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let id = huxley_orgs::create(&mut tx, &ctx, parent, kind, &unique(name))
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    id
}

async fn setup() -> Option<Setup> {
    let pools = pools().await?;
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(&pools).await;
    // The module is instance-wide: switch it on (other tests may have too).
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "crm", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Crm MSP").await;
    let acme = org(&pools, provider, huxley_orgs::OrgKind::Client, "Acme").await;
    let globex = org(&pools, provider, huxley_orgs::OrgKind::Client, "Globex").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        acme,
        globex,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("crm").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Crm User", PASSWORD)
        .await
        .unwrap();
    match role_at {
        None => huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap(),
        Some((role, org)) => {
            sqlx::query(
                "INSERT INTO role_assignments (id, user_id, role_id, org_id) \
                 SELECT $1, $2, id, $3 FROM roles WHERE key = $4 AND is_system",
            )
            .bind(Uuid::now_v7())
            .bind(u.id)
            .bind(org)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit().await.unwrap();
    (u.id, email)
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

impl Setup {
    async fn call(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
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
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn contact(&self, cookie: &str, org: Uuid, body: Value) -> Value {
        let (status, contact) = self
            .call(
                cookie,
                "POST",
                &format!("/api/v1/orgs/{org}/contacts"),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{contact}");
        contact
    }
}

#[tokio::test]
async fn company_details_and_sites() {
    let Some(s) = setup().await else { return };
    let company = format!("/api/v1/orgs/{}/company", s.acme);

    // Defaults until saved; saving needs the version read.
    let (status, details) = s.call(&s.owner, "GET", &company, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(details["relationship"], "client");
    assert_eq!(details["version"], 0);
    let save = |version: i64| {
        json!({ "relationship": "client", "status": "onboarding", "website": "https://acme.example",
                "phone": "+44 20 7946 0000", "version": version })
    };
    let (status, saved) = s.call(&s.owner, "PUT", &company, Some(save(0))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["status"], "onboarding");
    let (status, _) = s.call(&s.owner, "PUT", &company, Some(save(0))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            &company,
            Some(json!({ "relationship": "friend", "status": "active", "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");

    // The first site is the primary one; a new primary takes over.
    let sites = format!("/api/v1/orgs/{}/sites", s.acme);
    let (status, hq) = s
        .call(
            &s.owner,
            "POST",
            &sites,
            Some(json!({ "name": "HQ", "city": "London", "country": "gb", "timezone": "Europe/London",
                         "hours": { "mon": [["09:00", "17:30"]], "tue": [["09:00", "17:30"]] } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{hq}");
    assert_eq!(hq["isPrimary"], true);
    assert_eq!(hq["country"], "GB");
    let (_, depot) = s
        .call(
            &s.owner,
            "POST",
            &sites,
            Some(json!({ "name": "Depot", "timezone": "America/New_York", "isPrimary": true })),
        )
        .await;
    assert_eq!(depot["isPrimary"], true);
    let (_, listed) = s.call(&s.owner, "GET", &sites, None).await;
    let names: Vec<(&str, bool)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap(),
                s["isPrimary"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(names, [("Depot", true), ("HQ", false)]);

    for bad in [
        json!({ "name": "X", "timezone": "Mars/Base" }),
        json!({ "name": "X", "hours": { "mon": [["17:00", "09:00"]] } }),
        json!({ "name": "" }),
    ] {
        let (status, problem) = s.call(&s.owner, "POST", &sites, Some(bad.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad} {problem}");
    }

    // Updating needs the current version; archiving hides it.
    let site = format!("/api/v1/sites/{}", hq["id"].as_str().unwrap());
    let (status, moved) = s
        .call(
            &s.owner,
            "PUT",
            &site,
            Some(json!({ "name": "Head office", "timezone": "Europe/London", "version": hq["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["name"], "Head office");
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &site,
            Some(json!({ "name": "Stale", "version": hq["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        s.call(&s.owner, "DELETE", &site, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, listed) = s.call(&s.owner, "GET", &sites, None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn contacts_belong_to_a_primary_company_and_can_link_to_more() {
    let Some(s) = setup().await else { return };
    let ada = s
        .contact(
            &s.owner,
            s.acme,
            json!({ "firstName": "Ada", "lastName": "Lovelace", "email": "Ada@Acme.example",
                    "jobTitle": "CTO", "externalIds": { "entra": "00000000-0000-0000-0000-000000000001" } }),
        )
        .await;
    assert_eq!(ada["name"], "Ada Lovelace");
    assert_eq!(ada["email"], "ada@acme.example");
    assert_eq!(ada["companies"][0]["isPrimary"], true);
    assert_eq!(ada["companies"][0]["jobTitle"], "CTO");
    s.contact(
        &s.owner,
        s.acme,
        json!({ "firstName": "Grace", "lastName": "Hopper" }),
    )
    .await;

    // Listed and searchable at its company.
    let list = |org: Uuid, search: &str| format!("/api/v1/orgs/{org}/contacts?search={search}");
    let (_, found) = s.call(&s.owner, "GET", &list(s.acme, "love"), None).await;
    assert_eq!(found["total"], 1);
    assert_eq!(found["items"][0]["name"], "Ada Lovelace");
    let (_, all) = s.call(&s.owner, "GET", &list(s.acme, ""), None).await;
    assert_eq!(all["total"], 2);

    // Linked to Globex too, then Globex becomes the primary company.
    let id = ada["id"].as_str().unwrap();
    let links = format!("/api/v1/contacts/{id}/companies");
    let (status, linked) = s
        .call(
            &s.owner,
            "PUT",
            &links,
            Some(json!({ "version": ada["version"], "companies": [
                { "orgId": s.acme, "jobTitle": "CTO", "isPrimary": true },
                { "orgId": s.globex, "jobTitle": "Advisor" } ] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{linked}");
    let (_, at_globex) = s.call(&s.owner, "GET", &list(s.globex, ""), None).await;
    assert_eq!(at_globex["total"], 1);
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            &links,
            Some(json!({ "version": linked["version"], "companies": [
                { "orgId": s.acme }, { "orgId": s.globex } ] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let (_, moved) = s
        .call(
            &s.owner,
            "PUT",
            &links,
            Some(json!({ "version": linked["version"], "companies": [
                { "orgId": s.acme, "jobTitle": "CTO" },
                { "orgId": s.globex, "jobTitle": "Advisor", "isPrimary": true } ] })),
        )
        .await;
    assert_eq!(moved["orgId"], json!(s.globex));
    assert_eq!(moved["companies"][0]["orgId"], json!(s.globex));

    // Details change with the version; archiving drops it from lists.
    let contact = format!("/api/v1/contacts/{id}");
    let (status, renamed) = s
        .call(
            &s.owner,
            "PUT",
            &contact,
            Some(json!({ "firstName": "Augusta Ada", "lastName": "King", "version": moved["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Augusta Ada King");
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            &contact,
            Some(json!({ "firstName": "", "lastName": " ", "version": renamed["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(
        s.call(&s.owner, "DELETE", &contact, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, after) = s.call(&s.owner, "GET", &list(s.acme, ""), None).await;
    assert_eq!(after["total"], 1);

    // Every change is audited and emitted as an event.
    let (events,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM events_outbox WHERE subject_id = $1 AND event_type LIKE 'contact.%'",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert!(events >= 4, "{events}");
}

#[tokio::test]
async fn permissions_scope_and_the_module_gate() {
    let Some(s) = setup().await else { return };
    let (_, tech) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &tech).await;
    let (_, reader) = user(&s.pools, Some(("read_only", s.acme))).await;
    let reader = login(&s.app, &reader).await;
    let theirs = s
        .contact(&s.owner, s.globex, json!({ "firstName": "Hidden" }))
        .await;

    // A technician manages contacts at their client, not elsewhere.
    let mine = s
        .contact(&tech, s.acme, json!({ "firstName": "Tess" }))
        .await;
    let other = format!("/api/v1/contacts/{}", theirs["id"].as_str().unwrap());
    assert_eq!(
        s.call(&tech, "GET", &other, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = s
        .call(
            &tech,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.globex),
            Some(json!({ "firstName": "X" })),
        )
        .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );
    // Nor move a contact to a company they can't manage.
    let (status, _) = s
        .call(
            &tech,
            "PUT",
            &format!("/api/v1/contacts/{}/companies", mine["id"].as_str().unwrap()),
            Some(json!({ "version": mine["version"], "companies": [{ "orgId": s.globex, "isPrimary": true }] })),
        )
        .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{status}"
    );

    // Read-only people read, but don't change anything.
    let acme_contacts = format!("/api/v1/orgs/{}/contacts", s.acme);
    assert_eq!(
        s.call(&reader, "GET", &acme_contacts, None).await.0,
        StatusCode::OK
    );
    let (status, _) = s
        .call(
            &reader,
            "POST",
            &acme_contacts,
            Some(json!({ "firstName": "Nope" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn custom_fields_and_the_module_gate() {
    let Some(s) = setup().await else { return };
    let (_, tech) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &tech).await;
    let (_, reader) = user(&s.pools, Some(("read_only", s.acme))).await;
    let reader = login(&s.app, &reader).await;
    let mine = s
        .contact(&tech, s.acme, json!({ "firstName": "Tess" }))
        .await;
    let acme_contacts = format!("/api/v1/orgs/{}/contacts", s.acme);

    // Contacts take custom fields, guarded by crm:manage.
    let key = format!("tier_{}", Uuid::now_v7().simple());
    let (status, field) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/custom-fields",
            Some(json!({ "entityType": "contact", "key": key, "label": "Tier", "fieldType": "text" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    let fields = format!(
        "/api/v1/entities/contact/{}/fields",
        mine["id"].as_str().unwrap()
    );
    let (status, set) = s
        .call(
            &tech,
            "PATCH",
            &fields,
            Some(json!({ "values": { key.clone(): "gold" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let (status, _) = s
        .call(
            &reader,
            "PATCH",
            &fields,
            Some(json!({ "values": { key.clone(): "lead" } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Off for this provider and its clients: the routes don't exist there.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_for_org(&mut tx, &ctx, s.provider, "crm", false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, problem) = s.call(&s.owner, "GET", &acme_contacts, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        problem["type"],
        "https://usehuxley.io/problems/module-disabled"
    );
    assert_eq!(
        s.call(&s.owner, "GET", &fields, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/orgs/{}/company", s.acme),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_company_list_follows_permissions_and_the_module() {
    let Some(s) = setup().await else { return };
    let (_, tech) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &tech).await;
    s.contact(&tech, s.acme, json!({ "firstName": "Listed" }))
        .await;

    // A technician at Acme sees Acme only, with its counts.
    let (status, mine) = s.call(&tech, "GET", "/api/v1/companies", None).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let mine = mine.as_array().unwrap();
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0]["orgId"], json!(s.acme));
    assert_eq!(mine[0]["contacts"], 1);
    assert_eq!(mine[0]["relationship"], "client");

    // The owner finds it by name; providers only when asked.
    let (name,): (String,) = sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
        .bind(s.acme)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    let search = |extra: &str| {
        format!(
            "/api/v1/companies?search={}{extra}",
            name.replace(' ', "%20")
        )
    };
    let (_, found) = s.call(&s.owner, "GET", &search(""), None).await;
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    let (provider,): (String,) = sqlx::query_as("SELECT name FROM organizations WHERE id = $1")
        .bind(s.provider)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    let providers = format!(
        "/api/v1/companies?providers=true&search={}",
        provider.replace(' ', "%20")
    );
    let (_, listed) = s.call(&s.owner, "GET", &providers, None).await;
    assert_eq!(listed[0]["kind"], "provider", "{listed}");

    // Off for the provider: its clients leave the list.
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_for_org(&mut tx, &ctx, s.provider, "crm", false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (_, gone) = s.call(&s.owner, "GET", &search(""), None).await;
    assert_eq!(gone, json!([]));
    let (_, tech_list) = s.call(&tech, "GET", "/api/v1/companies", None).await;
    assert_eq!(tech_list, json!([]));
}
