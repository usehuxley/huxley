//! The `sales` module's API against a real PostgreSQL: stages and reasons,
//! opportunities moving through the pipeline, leads converting into a
//! client, a contact and an opportunity, permissions and the module gate.

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

const PASSWORD: &str = "sales api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
    acme: Uuid,
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
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "contracts", "sales"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Sales MSP").await;
    let acme = org(&pools, provider, huxley_orgs::OrgKind::Client, "Acme").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Some(Setup {
        pools,
        app,
        provider,
        acme,
        owner,
    })
}

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("sales").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Sales User", PASSWORD)
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

    async fn mv(&self, id: &str, body: Value) -> (StatusCode, Value) {
        self.call(
            &self.owner,
            "POST",
            &format!("/api/v1/opportunities/{id}/move"),
            Some(body),
        )
        .await
    }

    async fn stages(&self) -> Vec<Value> {
        let (status, stages) = self
            .call(&self.owner, "GET", "/api/v1/sales/stages", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{stages}");
        stages.as_array().unwrap().clone()
    }

    /// The first stage of a kind.
    async fn stage(&self, kind: &str) -> String {
        self.stages()
            .await
            .into_iter()
            .find(|s| s["kind"] == kind)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn events(&self, subject: &str, event: &str) -> Vec<Value> {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let rows: Vec<Value> = sqlx::query_scalar(
            "SELECT payload FROM events_outbox WHERE event_type = $1 AND subject_id = $2 ORDER BY occurred_at, id",
        )
        .bind(event)
        .bind(Uuid::parse_str(subject).unwrap())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        rows
    }
}

#[tokio::test]
async fn an_opportunity_moves_through_the_pipeline_and_is_won() {
    let Some(s) = setup().await else { return };
    let stages = s.stages().await;
    let open: Vec<&Value> = stages.iter().filter(|st| st["kind"] == "open").collect();
    assert!(open.len() >= 2);

    let (status, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": {
                "name": "Firewall refresh", "amount": "14820.00", "recurring": 450,
                "closeDate": "2027-01-31", "source": "referral" } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{o}");
    assert_eq!(o["stageId"], open[0]["id"]);
    assert_eq!(o["status"], "open");
    assert_eq!(o["amount"], "14820");
    // (14820 + 12 × 450) × the first stage's probability.
    let p = open[0]["probability"].as_f64().unwrap();
    let weighted: f64 = o["weighted"].as_str().unwrap().parse().unwrap();
    assert!((weighted - (14820.0 + 5400.0) * p / 100.0).abs() < 0.01);
    let id = o["id"].as_str().unwrap();

    // Moves keep history; a stale version is refused.
    let (status, moved) = s
        .mv(
            id,
            json!({ "stageId": open[1]["id"], "version": o["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let (status, _) = s
        .mv(
            id,
            json!({ "stageId": open[0]["id"], "version": o["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Won with a reason of the right kind, and its probability is 100.
    let (_, reasons) = s.call(&s.owner, "GET", "/api/v1/sales/reasons", None).await;
    let reason = |kind: &str| {
        reasons
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == kind)
            .unwrap()["id"]
            .clone()
    };
    let won = s.stage("won").await;
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/opportunities/{id}/move"),
            Some(
                json!({ "stageId": won, "reasonId": reason("lost"), "version": moved["version"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let (status, closed) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/opportunities/{id}/move"),
            Some(
                json!({ "stageId": won, "reasonId": reason("won"), "reasonNote": "Best support",
                         "version": moved["version"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(closed["status"], "won");
    assert_eq!(closed["probability"], 100);
    assert!(closed["closedAt"].is_string());
    assert_eq!(closed["reasonNote"], "Best support");

    let (_, history) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/opportunities/{id}/history"),
            None,
        )
        .await;
    let steps: Vec<(&Value, &Value)> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (&h["fromStageId"], &h["toStageId"]))
        .collect();
    assert_eq!(steps.len(), 3, "{history}");
    assert_eq!(steps[0].0, &Value::Null);
    assert_eq!(steps[2].1.as_str().unwrap(), won);

    assert_eq!(s.events(id, "sales.opportunity.created").await.len(), 1);
    assert_eq!(
        s.events(id, "sales.opportunity.stage_changed").await.len(),
        2
    );
    let won_events = s.events(id, "sales.opportunity.won").await;
    assert_eq!(won_events.len(), 1);
    assert_eq!(won_events[0]["amount"], "14820");
}

#[tokio::test]
async fn a_closed_opportunity_reopens_and_lists_by_status() {
    let Some(s) = setup().await else { return };
    let (_, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": "Backup appliance" } })),
        )
        .await;
    let id = o["id"].as_str().unwrap();
    let open = s.stage("open").await;
    let (status, closed) = s
        .mv(
            id,
            json!({ "stageId": s.stage("lost").await, "version": o["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{closed}");
    assert_eq!(closed["status"], "lost");
    assert_eq!(s.events(id, "sales.opportunity.lost").await.len(), 1);

    // Reopening moves it back to an open stage and clears the close.
    let (status, reopened) = s
        .mv(id, json!({ "stageId": open, "version": closed["version"] }))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reopened["status"], "open");
    assert_eq!(reopened["closedAt"], Value::Null);

    // Listed for the client, and filtered by status.
    let (_, listed) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/opportunities?orgId={}&status=open", s.acme),
            None,
        )
        .await;
    assert!(listed.as_array().unwrap().iter().any(|x| x["id"] == id));
    let (_, lost) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/opportunities?orgId={}&status=lost", s.acme),
            None,
        )
        .await;
    assert!(!lost.as_array().unwrap().iter().any(|x| x["id"] == id));
}

#[tokio::test]
async fn a_lead_converts_into_a_client_a_contact_and_an_opportunity() {
    let Some(s) = setup().await else { return };
    let company = unique("Initech");
    let (status, lead) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/leads",
            Some(json!({ "orgId": s.provider, "lead": {
                "companyName": company, "firstName": "Peter", "lastName": "Gibbons",
                "email": "peter@initech.example", "source": "web form" } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{lead}");
    assert_eq!(lead["status"], "new");
    let id = lead["id"].as_str().unwrap();
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/leads",
            Some(json!({ "orgId": s.provider, "lead": { "companyName": "X", "email": "nope" } })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");

    let (status, done) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{id}/convert"),
            Some(
                json!({ "opportunity": { "name": "Managed services", "recurring": "2400" },
                         "version": lead["version"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["lead"]["status"], "converted");
    let client = done["lead"]["convertedOrgId"].as_str().unwrap();
    assert_eq!(done["opportunity"]["orgId"], client);
    assert_eq!(done["opportunity"]["source"], "web form");
    assert_eq!(done["opportunity"]["contactName"], "Peter Gibbons");
    assert_eq!(done["opportunity"]["leadId"], id);

    // The client is a new prospect under the provider.
    let (_, org) = s
        .call(&s.owner, "GET", &format!("/api/v1/orgs/{client}"), None)
        .await;
    assert_eq!(org["name"], company);
    assert_eq!(org["parentId"], s.provider.to_string());
    let (_, details) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/orgs/{client}/company"),
            None,
        )
        .await;
    assert_eq!(details["relationship"], "prospect");
    assert_eq!(s.events(id, "sales.lead.converted").await.len(), 1);

    // Converted once only; a second lead for the same company goes into it.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{id}/convert"),
            Some(json!({ "opportunity": { "name": "Again" }, "version": done["lead"]["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, again) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/leads",
            Some(json!({ "orgId": s.provider, "lead": { "companyName": company } })),
        )
        .await;
    let again_id = again["id"].as_str().unwrap();
    let (status, clash) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{again_id}/convert"),
            Some(json!({ "opportunity": { "name": "Printers" }, "version": again["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{clash}");
    let (status, into) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{again_id}/convert"),
            Some(
                json!({ "orgId": client, "opportunity": { "name": "Printers" },
                         "version": again["version"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{into}");
    assert_eq!(into["opportunity"]["orgId"], client);
    assert_eq!(into["contactId"], Value::Null);
}

#[tokio::test]
async fn a_disqualified_lead_says_why_and_is_not_converted() {
    let Some(s) = setup().await else { return };
    let (_, cold) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/leads",
            Some(json!({ "orgId": s.provider, "lead": { "companyName": unique("Cold") } })),
        )
        .await;
    let cold_id = cold["id"].as_str().unwrap();
    let (status, dq) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{cold_id}/disqualify"),
            Some(json!({ "reason": "Student project", "version": cold["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{dq}");
    assert_eq!(dq["status"], "disqualified");
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/leads/{cold_id}/convert"),
            Some(json!({ "opportunity": { "name": "X" }, "version": dq["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn stages_are_kept_by_configurers_and_guard_their_kinds() {
    let Some(s) = setup().await else { return };
    let stages = s.stages().await;
    let won = |kind: &str, p: i64| {
        stages
            .iter()
            .any(|st| st["kind"] == kind && st["probability"] == p)
    };
    assert!(won("won", 100) && won("lost", 0));
    let name = unique("Demo booked");
    let (status, stage) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/sales/stages",
            Some(json!({ "name": name, "kind": "open", "probability": 35 })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{stage}");
    let id = stage["id"].as_str().unwrap();
    // Won stages are always 100.
    let (_, changed) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/sales/stages/{id}"),
            Some(json!({ "name": name, "kind": "open", "probability": 140 })),
        )
        .await;
    assert_eq!(changed["errors"][0]["pointer"], "/probability");

    // An open opportunity there stops it being archived.
    let (_, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": "Demo", "stageId": id } })),
        )
        .await;
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/sales/stages/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let lost = s.stage("lost").await;
    s.call(
        &s.owner,
        "POST",
        &format!("/api/v1/opportunities/{}/move", o["id"].as_str().unwrap()),
        Some(json!({ "stageId": lost, "version": o["version"] })),
    )
    .await;
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/sales/stages/{id}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Starting at a closed stage isn't allowed.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": "X", "stageId": lost } })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn sales_people_work_deals_and_others_only_look() {
    let Some(s) = setup().await else { return };
    let (_, rep) = user(&s.pools, Some(("sales", s.provider))).await;
    let rep = login(&s.app, &rep).await;
    let (_, viewer) = user(&s.pools, Some(("read_only", s.provider))).await;
    let viewer = login(&s.app, &viewer).await;
    let (_, tech) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech).await;

    let new = json!({ "orgId": s.acme, "opportunity": { "name": "Laptops" } });
    let (status, o) = s
        .call(&rep, "POST", "/api/v1/opportunities", Some(new.clone()))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{o}");
    let path = format!("/api/v1/opportunities/{}", o["id"].as_str().unwrap());

    // Read-only users see it, but can't change it.
    let (status, _) = s.call(&viewer, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = s
        .call(&viewer, "POST", "/api/v1/opportunities", Some(new.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Technicians don't see sales.
    let (status, _) = s.call(&tech, "GET", &path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, none) = s.call(&tech, "GET", "/api/v1/opportunities", None).await;
    assert!(!none.as_array().unwrap().iter().any(|x| x["id"] == o["id"]));
    // Only instance configurers keep stages.
    let (status, _) = s
        .call(
            &rep,
            "POST",
            "/api/v1/sales/stages",
            Some(json!({ "name": unique("Nope"), "kind": "open", "probability": 5 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s.call(&rep, "GET", "/api/v1/sales/stages", None).await;
    assert_eq!(status, StatusCode::OK);
}

impl Setup {
    async fn product(&self, sku: &str, cost: &str, price: &str, billing: &str) -> String {
        let (status, p) = self
            .call(
                &self.owner,
                "POST",
                "/api/v1/catalog/products",
                Some(json!({ "orgId": self.provider, "product": {
                    "sku": unique(sku).replace(' ', "-"), "name": sku, "kind": "product",
                    "cost": cost, "price": price, "billing": billing, "unit": "each" } })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{p}");
        p["id"].as_str().unwrap().to_owned()
    }

    async fn quote(&self, body: Value) -> Value {
        let (status, q) = self
            .call(&self.owner, "POST", "/api/v1/quotes", Some(body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{q}");
        q
    }
}

#[tokio::test]
async fn a_quote_is_priced_from_the_catalog_sent_revised_and_accepted() {
    let Some(s) = setup().await else { return };
    let firewall = s.product("FG-60F", "800", "1000", "one_time").await;
    let (_, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": "Firewall refresh" } })),
        )
        .await;
    let q = s
        .quote(json!({ "orgId": s.acme, "quote": {
            "title": "Firewall refresh", "opportunityId": o["id"], "validUntil": "2027-01-31",
            "lines": [
                { "kind": "heading", "name": "Hardware" },
                { "productId": firewall, "quantity": 2, "discountPct": 10 },
                { "name": "Managed firewall", "billing": "monthly", "quantity": 2,
                  "unitCost": "15", "unitPrice": "25" },
                { "name": "Spare unit", "optional": true, "selected": false, "unitCost": "800", "unitPrice": "1000" }
            ] } }))
        .await;
    // The catalog filled the firewall line in.
    assert_eq!(q["lines"][1]["name"], "FG-60F");
    assert_eq!(q["lines"][1]["unitPrice"], "1000");
    assert_eq!(q["lines"][1]["total"], "1800");
    assert_eq!(q["totals"]["oneTime"], "1800");
    assert_eq!(q["totals"]["monthly"], "50");
    assert_eq!(q["totals"]["firstYear"], "2400");
    // (2400 - (1600 + 360)) / 2400 = 18.3%.
    assert_eq!(q["totals"]["marginPct"], "18.3");
    let id = q["id"].as_str().unwrap();

    // Sending keeps revision 1; changing it after starts revision 2.
    let (status, sent) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(json!({ "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["status"], "sent");
    let mut body = json!({ "title": "Firewall refresh", "opportunityId": o["id"],
        "lines": q["lines"].as_array().unwrap().iter().map(|l| json!({
            "kind": l["kind"], "name": l["name"], "productId": l["productId"], "billing": l["billing"],
            "quantity": l["quantity"], "unitCost": l["unitCost"], "unitPrice": l["unitPrice"],
            "discountPct": l["discountPct"], "optional": l["optional"] })).collect::<Vec<_>>(),
        "version": sent["version"] });
    body["lines"][1]["discountPct"] = json!("5");
    let (status, revised) = s
        .call(&s.owner, "PUT", &format!("/api/v1/quotes/{id}"), Some(body))
        .await;
    assert_eq!(status, StatusCode::OK, "{revised}");
    assert_eq!(revised["status"], "draft");
    assert_eq!(revised["revision"], 2);
    let (_, sent) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(json!({ "version": revised["version"] })),
        )
        .await;
    let (_, revisions) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/quotes/{id}/revisions"),
            None,
        )
        .await;
    let revs = revisions.as_array().unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[1]["snapshot"]["lines"][1]["discountPct"], "10");
    assert_eq!(revs[0]["snapshot"]["lines"][1]["discountPct"], "5");

    // Accepted with the option: it counts, and the opportunity is won.
    let option = sent["lines"][3]["id"].clone();
    let (status, accepted) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/decision"),
            Some(json!({ "accept": true, "name": "Rosa Park", "options": [option], "version": sent["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["decidedBy"], "Rosa Park");
    assert_eq!(accepted["totals"]["oneTime"], "2900");
    let (_, won) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/opportunities/{}", o["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(won["status"], "won");
    assert_eq!(s.events(id, "sales.quote.accepted").await.len(), 1);
    assert_eq!(s.events(id, "sales.quote.sent").await.len(), 2);
}

#[tokio::test]
async fn a_quote_is_declined_and_printed() {
    let Some(s) = setup().await else { return };
    let q = s
        .quote(json!({ "orgId": s.acme, "quote": { "title": "Printers",
            "lines": [{ "name": "Printer", "quantity": 3, "unitPrice": "450" }] } }))
        .await;
    let id = q["id"].as_str().unwrap();
    // Not answerable until sent; empty quotes can't be sent.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/decision"),
            Some(json!({ "accept": true, "name": "X", "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let empty = s
        .quote(json!({ "orgId": s.acme, "quote": { "title": "Nothing yet" } }))
        .await;
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{}/send", empty["id"].as_str().unwrap()),
            Some(json!({ "version": empty["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/quotes/{}", empty["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, sent) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(json!({ "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let (status, declined) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/decision"),
            Some(
                json!({ "accept": false, "name": "Tom Hale", "reason": "Too expensive",
                           "version": sent["version"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{declined}");
    assert_eq!(declined["status"], "declined");
    assert_eq!(declined["declineReason"], "Too expensive");

    // Decided quotes don't change, and sent ones stay on record.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/quotes/{id}"),
            Some(json!({ "title": "X", "version": declined["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = s
        .call(&s.owner, "DELETE", &format!("/api/v1/quotes/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let req = Request::get(format!("/api/v1/quotes/{id}/pdf"))
        .header("x-requested-with", "huxley")
        .header("cookie", &s.owner)
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/pdf");
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.starts_with(b"%PDF-"));
}

impl Setup {
    /// A rule matching only quotes titled `title` (rules are instance-wide).
    async fn rule_for(&self, title: &str) -> String {
        let (status, rule) = self
            .call(
                &self.owner,
                "POST",
                "/api/v1/sales/review-rules",
                Some(json!({ "name": "This test's quotes", "kind": "expression",
                    "expression": format!("quote.title == '{title}'"), "reviewers": "role:admin" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{rule}");
        rule["id"].as_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn a_quote_that_needs_review_waits_for_it_or_an_override() {
    let Some(s) = setup().await else { return };
    let title = unique("Review me");
    let rule = s.rule_for(&title).await;
    let (_, rep) = user(&s.pools, Some(("sales", s.provider))).await;
    let rep = login(&s.app, &rep).await;
    let q = s
        .quote(json!({ "orgId": s.acme, "quote": { "title": title,
            "lines": [{ "name": "Server", "unitCost": "900", "unitPrice": "1000" }] } }))
        .await;
    assert_eq!(q["reviewStatus"], "needed");
    let id = q["id"].as_str().unwrap();
    let (status, review) = s
        .call(&rep, "GET", &format!("/api/v1/quotes/{id}/review"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["reasons"][0]["name"], "This test's quotes");
    assert_eq!(review["canSend"], false);
    assert_eq!(review["canOverride"], false);

    // Not without review; and only some may send it anyway.
    let body = |over: bool| json!({ "version": q["version"], "overrideReview": over });
    let (status, _) = s
        .call(
            &rep,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(body(false)),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = s
        .call(
            &rep,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(body(true)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Submitted: in review, with the event for the workflow.
    let (status, review) = s
        .call(
            &rep,
            "POST",
            &format!("/api/v1/quotes/{id}/review"),
            Some(json!({ "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["status"], "in_review");
    let events = s.events(id, "quote.submitted_for_review").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["reviewers"], json!(["role:admin"]));
    assert_eq!(
        events[0]["reasons"][0],
        format!("This test's quotes: matches quote.title == '{title}'")
    );
    let (status, _) = s
        .call(
            &rep,
            "POST",
            &format!("/api/v1/quotes/{id}/review"),
            Some(json!({ "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // An administrator sends it anyway; that's on record.
    let (status, sent) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/quotes/{id}/send"),
            Some(body(true)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let (_, review) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/quotes/{id}/review"),
            None,
        )
        .await;
    assert_eq!(review["sentOverridden"], true);
    s.call(
        &s.owner,
        "DELETE",
        &format!("/api/v1/sales/review-rules/{rule}"),
        None,
    )
    .await;
}

#[tokio::test]
async fn people_comment_on_quote_lines_and_mention_each_other() {
    let Some(s) = setup().await else { return };
    let (rep_id, rep) = user(&s.pools, Some(("sales", s.provider))).await;
    let rep = login(&s.app, &rep).await;
    let (boss, _) = user(&s.pools, None).await;
    let q = s
        .quote(json!({ "orgId": s.acme, "quote": { "title": "Switches",
            "lines": [{ "name": "Switch", "quantity": 4, "unitPrice": "300" }] } }))
        .await;
    let id = q["id"].as_str().unwrap();
    let line = q["lines"][0]["id"].clone();
    let path = format!("/api/v1/quotes/{id}/comments");
    let (status, c) = s
        .call(
            &rep,
            "POST",
            &path,
            Some(json!({ "body": "Is 300 right?", "lineId": line, "mentions": [boss] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{c}");
    assert_eq!(c["lineName"], "Switch");
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &path,
            Some(json!({ "body": "Yes", "parentId": c["id"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    // Replies go on a thread's first comment; lines must be the quote's.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &path,
            Some(json!({ "body": "x", "lineId": Uuid::now_v7() })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // The boss was told; the author wasn't.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let told: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, link FROM notifications WHERE kind = 'quote.mentioned' AND link LIKE $1",
    )
    .bind(format!("/sales/quotes/{id}%"))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].0, boss);
    assert_ne!(told[0].0, rep_id);

    // Saving the quote keeps the line (and its comments' anchor).
    let (status, saved) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/quotes/{id}"),
            Some(json!({ "title": "Switches",
            "lines": [{ "id": line, "name": "Switch", "quantity": 5, "unitPrice": "300" }],
            "version": q["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["lines"][0]["id"], line);

    let (status, _) = s
        .call(
            &rep,
            "POST",
            &format!("{path}/{}/resolve", c["id"].as_str().unwrap()),
            Some(json!({ "resolved": true })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, all) = s.call(&rep, "GET", &path, None).await;
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[0]["resolvedAt"].is_string());
    assert_eq!(all[1]["parentId"], c["id"]);
}

#[tokio::test]
async fn review_rules_are_kept_by_configurers() {
    let Some(s) = setup().await else { return };
    let rule = s.rule_for(&unique("Nobody uses this title")).await;
    let (_, rep) = user(&s.pools, Some(("sales", s.provider))).await;
    let rep = login(&s.app, &rep).await;
    // Rules are kept by configurers only.
    let (status, _) = s
        .call(
            &rep,
            "DELETE",
            &format!("/api/v1/sales/review-rules/{rule}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, bad) = s
        .call(
            &s.owner,
            "PUT",
            &format!("/api/v1/sales/review-rules/{rule}"),
            Some(json!({ "name": "Bad", "kind": "margin_below", "reviewers": "admins" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    let (status, _) = s
        .call(
            &s.owner,
            "DELETE",
            &format!("/api/v1/sales/review-rules/{rule}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_forecast_compares_won_and_pipeline_with_quota() {
    let Some(s) = setup().await else { return };
    // A salesperson of their own, so other tests' deals don't count.
    let (rep, _) = user(&s.pools, Some(("sales", s.provider))).await;
    let today = time::OffsetDateTime::now_utc().date();
    let q_month = (u8::from(today.month()) - 1) / 3 * 3 + 1;
    let start =
        time::Date::from_calendar_date(today.year(), time::Month::try_from(q_month).unwrap(), 1)
            .unwrap();
    let end = start
        .replace_month(time::Month::try_from(q_month + 2).unwrap())
        .unwrap()
        .replace_day(28)
        .unwrap();
    let deal = |name: &str, amount: &str, recurring: &str, forecast: &str| {
        json!({ "orgId": s.acme, "opportunity": { "name": unique(name), "amount": amount, "recurring": recurring,
            "forecast": forecast, "ownerId": rep, "closeDate": end.to_string(), "probability": 50 } })
    };
    for (name, amount, recurring, forecast) in [
        ("Firewalls", "1000", "100", "commit"),
        ("Backup", "500", "0", "best_case"),
        ("Phones", "300", "0", "pipeline"),
        ("Moonshot", "9999", "0", "omitted"),
    ] {
        let (status, o) = s
            .call(
                &s.owner,
                "POST",
                "/api/v1/opportunities",
                Some(deal(name, amount, recurring, forecast)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{o}");
    }
    let (_, won) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(deal("Laptops", "4000", "0", "commit")),
        )
        .await;
    let (status, w) = s
        .mv(
            won["id"].as_str().unwrap(),
            json!({ "stageId": s.stage("won").await, "version": won["version"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{w}");
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/sales/quotas",
            Some(json!({ "orgId": s.provider, "userId": rep, "periodStart": start.to_string(), "months": 3,
                "amount": "10000" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, rows) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/sales/forecast?from={start}&to={end}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let mine = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["ownerId"] == json!(rep))
        .unwrap();
    assert_eq!(mine["quota"], "10000");
    assert_eq!(mine["won"], "4000");
    assert_eq!(mine["attainment"], "40");
    // Firewalls: 1000 + 12 × 100.
    assert_eq!(mine["commit"], "2200");
    assert_eq!(mine["bestCase"], "2700");
    assert_eq!(mine["pipeline"], "3000");
    assert_eq!(mine["weighted"], "1500");
    assert_eq!(mine["openCount"], 3);
}

#[tokio::test]
async fn quotas_are_set_by_month_or_quarter() {
    let Some(s) = setup().await else { return };
    let (rep, _) = user(&s.pools, Some(("sales", s.provider))).await;
    let today = time::OffsetDateTime::now_utc().date();
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/sales/quotas",
            Some(json!({ "orgId": s.provider, "userId": rep, "periodStart": format!("{}-04-01", today.year()),
                "months": 3, "amount": "9000" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Quarters start in January, April, July or October.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            "/api/v1/sales/quotas",
            Some(json!({ "orgId": s.provider, "userId": rep, "periodStart": format!("{}-02-01", today.year()),
                "months": 3, "amount": "1" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, quotas) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/sales/quotas?year={}", today.year()),
            None,
        )
        .await;
    assert!(
        quotas
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["userId"] == json!(rep))
    );
}

#[tokio::test]
async fn a_scope_is_written_approved_and_carried_by_the_quote() {
    let Some(s) = setup().await else { return };
    let (_, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": unique("Rollout") } })),
        )
        .await;
    let opp = o["id"].as_str().unwrap().to_owned();
    let (status, scope) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/opportunities/{opp}/scope"),
            Some(json!({ "template": "project" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{scope}");
    assert_eq!(scope["sections"].as_array().unwrap().len(), 9);
    let id = scope["id"].as_str().unwrap().to_owned();
    let path = |p: &str| format!("/api/v1/scopes/{id}{p}");

    s.call(
        &s.owner,
        "POST",
        &path("/sources"),
        Some(json!({ "kind": "note", "title": "Site visit", "body": "Two floors, 30 desks." })),
    )
    .await;
    s.call(
        &s.owner,
        "PUT",
        &path("/sections/in_scope"),
        Some(json!({ "body": "- 30 desks across two floors" })),
    )
    .await;
    let (_, scope) = s
        .call(
            &s.owner,
            "POST",
            &path("/questions"),
            Some(json!({ "sectionKey": "timeline", "question": "Go-live date?" })),
        )
        .await;
    let q = scope["questions"][0]["id"].as_str().unwrap().to_owned();
    let (_, scope) = s
        .call(
            &s.owner,
            "POST",
            &path(&format!("/questions/{q}")),
            Some(json!({ "answer": "March", "resolved": true })),
        )
        .await;
    assert!(scope["questions"][0]["resolvedAt"].is_string());

    let (status, approved) = s
        .call(
            &s.owner,
            "POST",
            &path("/approve"),
            Some(json!({ "version": scope["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let version = approved["versions"][0]["id"].clone();
    assert_eq!(approved["versions"][0]["number"], 1);

    // A quote on the opportunity carries it; another opportunity's can't.
    let q = s
        .quote(json!({ "orgId": s.acme, "quote": { "title": "Rollout", "opportunityId": opp, "scopeVersionId": version,
            "lines": [{ "name": "Desk setup", "quantity": 30, "unitPrice": "50" }] } }))
        .await;
    assert_eq!(q["scope"]["sections"][0]["title"], "In scope");
    assert_eq!(
        q["scope"]["sections"][0]["body"],
        "- 30 desks across two floors"
    );
    let (status, _) = s
        .call(&s.owner, "POST", "/api/v1/quotes", Some(json!({ "orgId": s.acme, "quote": { "title": "Other", "scopeVersionId": version } })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // What's sent includes the scope, so it's agreed to with the quote.
    let qid = q["id"].as_str().unwrap();
    s.call(
        &s.owner,
        "POST",
        &format!("/api/v1/quotes/{qid}/send"),
        Some(json!({ "version": q["version"] })),
    )
    .await;
    let (_, revs) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/quotes/{qid}/revisions"),
            None,
        )
        .await;
    assert_eq!(revs[0]["snapshot"]["scope"]["number"], 1);
}

#[tokio::test]
async fn a_scope_draft_needs_discovery_and_can_be_called_off() {
    let Some(s) = setup().await else { return };
    let (_, o) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/opportunities",
            Some(json!({ "orgId": s.acme, "opportunity": { "name": unique("Audit") } })),
        )
        .await;
    let opp = o["id"].as_str().unwrap().to_owned();
    let (status, _) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/opportunities/{opp}/scope"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, scope) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/opportunities/{opp}/scope"),
            Some(json!({ "template": "assessment" })),
        )
        .await;
    let id = scope["id"].as_str().unwrap().to_owned();
    let path = |p: &str| format!("/api/v1/scopes/{id}{p}");
    // No draft without discovery.
    let (status, _) = s.call(&s.owner, "POST", &path("/draft"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    s.call(
        &s.owner,
        "POST",
        &path("/sources"),
        Some(json!({ "kind": "note", "title": "Site visit", "body": "Two floors, 30 desks." })),
    )
    .await;
    let (status, drafting) = s.call(&s.owner, "POST", &path("/draft"), None).await;
    assert_eq!(status, StatusCode::OK, "{drafting}");
    assert_eq!(drafting["draftState"], "requested");
    let (_, scope) = s.call(&s.owner, "DELETE", &path("/draft"), None).await;
    assert_eq!(scope["draftState"], "idle");
}
