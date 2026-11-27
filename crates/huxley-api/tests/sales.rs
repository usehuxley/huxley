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
