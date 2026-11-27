//! Expenses (ADR-079): technicians log mileage and receipts against tickets
//! and projects; categories set defaults and policy; entries are their own.

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

const PASSWORD: &str = "billing api passphrase";

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
    for module in [
        "crm",
        "servicedesk",
        "projects",
        "contracts",
        "billing",
        "expenses",
    ] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Spend MSP").await;
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
    let email = format!("{}@example.com", unique("stock").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Stock User", PASSWORD)
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

    async fn ok(
        &self,
        cookie: &str,
        method: &str,
        uri: &str,
        body: Value,
        want: StatusCode,
    ) -> Value {
        let (status, v) = self.call(cookie, method, uri, Some(body)).await;
        assert_eq!(status, want, "{method} {uri}: {v}");
        v
    }
}

impl Setup {
    /// Uploads a receipt as `cookie`.
    async fn upload(
        &self,
        cookie: &str,
        id: &str,
        name: &str,
        kind: &str,
        data: &[u8],
    ) -> (StatusCode, Value) {
        let boundary = "huxleyboundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {kind}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let req = Request::put(format!("/api/v1/expenses/{id}/receipt"))
            .header("x-requested-with", "huxley")
            .header("cookie", cookie)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Approves a report, as its approval workflow would.
    async fn approve(&self, report: &str) {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let ctx = RequestContext::system(Via::System);
        let id = Uuid::parse_str(report).unwrap();
        huxley_expenses::reports::decide(
            &mut tx,
            &ctx,
            id,
            huxley_expenses::reports::Decision::Approved,
            "Lead",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// What's unbilled for Acme today.
    async fn unbilled(&self, today: &str) -> Value {
        let uri = format!(
            "/api/v1/billing/unbilled?orgId={}&from={today}&to={today}&clients={}",
            self.provider, self.acme
        );
        self.ok(&self.owner, "GET", &uri, json!(null), StatusCode::OK)
            .await
    }

    async fn category(&self, name: &str) -> String {
        let (_, all) = self
            .call(&self.owner, "GET", "/api/v1/expense-categories", None)
            .await;
        all.as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

#[tokio::test]
async fn a_technician_logs_mileage_a_receipt_and_a_hotel() {
    let Some(s) = setup().await else { return };
    let (_, tech_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech_email).await;
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let ticket = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            json!({ "orgId": s.acme, "title": unique("Server swap") }),
            StatusCode::CREATED,
        )
        .await;

    // Mileage at the rate in effect: 12.5 miles at 0.70.
    let mileage = s.category("Mileage").await;
    let drive = s
        .ok(&tech, "POST", "/api/v1/expenses",
            json!({ "categoryId": mileage, "spentOn": today, "description": "To the client and back",
                    "distance": "12.5", "ticketId": ticket["id"] }), StatusCode::CREATED)
        .await;
    assert_eq!(drive["amount"], "8.75");
    assert_eq!(drive["rate"], "0.7");
    assert_eq!(drive["orgId"], json!(s.acme));
    assert_eq!(drive["billable"], true);
    assert_eq!(drive["flags"], json!([]));

    // A parts receipt: flagged until the receipt is there.
    let parts = s.category("Parts and supplies").await;
    let disk = s
        .ok(&tech, "POST", "/api/v1/expenses",
            json!({ "categoryId": parts, "spentOn": today, "description": "Replacement SSD", "vendor": "Micro Center",
                    "amount": "129.99", "tax": "9.10", "ticketId": ticket["id"] }), StatusCode::CREATED)
        .await;
    assert_eq!(disk["flags"], json!(["no_receipt"]));
    let id = disk["id"].as_str().unwrap();
    let (status, _) = s
        .upload(&tech, id, "evil.svg", "image/svg+xml", b"<svg/>")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, with) = s
        .upload(&tech, id, "receipt.jpg", "image/jpeg", b"\xff\xd8jpeg")
        .await;
    assert_eq!(status, StatusCode::OK, "{with}");
    assert_eq!(with["flags"], json!([]));
    assert_eq!(with["receiptName"], "receipt.jpg");

    // A hotel on a project, marked up 10%.
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            json!({ "orgId": s.acme, "name": unique("Office move"), "startOn": today }),
            StatusCode::CREATED,
        )
        .await;
    let travel = s.category("Travel and lodging").await;
    let hotel = s
        .ok(&tech, "POST", "/api/v1/expenses",
            json!({ "categoryId": travel, "spentOn": today, "description": "Hotel, one night", "amount": "180",
                    "markupPct": "10", "projectId": project["id"] }), StatusCode::CREATED)
        .await;
    assert_eq!(hotel["billAmount"], "198");
    assert_eq!(hotel["projectName"], project["name"]);

    // The ticket shows its expenses; the technician sees their own.
    let (_, on_ticket) = s
        .call(
            &s.owner,
            "GET",
            &format!(
                "/api/v1/expenses?ticketId={}",
                ticket["id"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(on_ticket.as_array().unwrap().len(), 2);
    let (_, mine) = s.call(&tech, "GET", "/api/v1/expenses", None).await;
    assert_eq!(mine.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn entries_are_their_owners_and_categories_are_configured() {
    let Some(s) = setup().await else { return };
    let (_, a_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let (_, b_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let (a, b) = (login(&s.app, &a_email).await, login(&s.app, &b_email).await);
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let other = s.category("Other").await;
    let lunch = s
        .ok(&a, "POST", "/api/v1/expenses",
            json!({ "categoryId": other, "spentOn": today, "description": "Cables", "amount": "12", "orgId": s.provider }),
            StatusCode::CREATED)
        .await;
    let id = lunch["id"].as_str().unwrap();
    // Not B's to see or change.
    let (status, _) = s
        .call(&b, "GET", &format!("/api/v1/expenses/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = s
        .call(&b, "DELETE", &format!("/api/v1/expenses/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, theirs) = s.call(&b, "GET", "/api/v1/expenses", None).await;
    assert_eq!(theirs.as_array().unwrap().len(), 0);
    // Tomorrow is refused; mileage needs a distance.
    let tomorrow = (time::OffsetDateTime::now_utc().date() + time::Duration::days(1)).to_string();
    let (status, _) = s
        .call(&a, "POST", "/api/v1/expenses",
            Some(json!({ "categoryId": other, "spentOn": tomorrow, "description": "x", "amount": "1", "orgId": s.provider })))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Categories: technicians read them; only configurers add them.
    let (status, _) = s
        .call(
            &a,
            "POST",
            "/api/v1/expense-categories",
            Some(json!({ "name": unique("Tolls") })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let tolls = s
        .ok(&s.owner, "POST", "/api/v1/expense-categories",
            json!({ "name": unique("Tolls"), "billable": true, "limitAmount": "20", "receiptRequired": false }),
            StatusCode::CREATED)
        .await;
    let toll = s
        .ok(&a, "POST", "/api/v1/expenses",
            json!({ "categoryId": tolls["id"], "spentOn": today, "description": "Bridge", "amount": "25", "orgId": s.acme }),
            StatusCode::CREATED)
        .await;
    assert_eq!(toll["flags"], json!(["over_limit"]));
    assert_eq!(toll["billable"], true);
    // A draft is deleted by its owner.
    let (status, _) = s
        .call(&a, "DELETE", &format!("/api/v1/expenses/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn approved_billable_expenses_are_invoiced_and_count_on_the_project() {
    let Some(s) = setup().await else { return };
    let (_, tech_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech_email).await;
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            json!({ "orgId": s.acme, "name": unique("Rollout"), "startOn": today }),
            StatusCode::CREATED,
        )
        .await;
    let travel = s.category("Travel and lodging").await;
    let meals = s.category("Meals").await;
    let mut ids = Vec::new();
    for (category, what, amount, billable) in [
        (&travel, "Hotel", "180", true),
        (&meals, "Dinner", "40", false),
    ] {
        let e = s
            .ok(&tech, "POST", "/api/v1/expenses",
                json!({ "categoryId": category, "spentOn": today, "description": what, "amount": amount,
                        "billable": billable, "markupPct": "10", "projectId": project["id"] }),
                StatusCode::CREATED)
            .await;
        ids.push(e["id"].clone());
    }
    let report = s
        .ok(
            &tech,
            "POST",
            "/api/v1/expense-reports",
            json!({ "entryIds": ids }),
            StatusCode::CREATED,
        )
        .await;
    // Submitted expenses count on the project's cost.
    let p = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/projects/{}", project["id"].as_str().unwrap()),
            json!(null),
            StatusCode::OK,
        )
        .await;
    assert_eq!(p["expenseCost"], 220.0);
    // Not billed until approved.
    let rate = json!({ "orgId": s.provider, "hourlyRate": "100", "currency": "USD", "paymentDays": 30, "approvedTimeOnly": false });
    s.ok(
        &s.owner,
        "PUT",
        "/api/v1/billing/settings",
        rate,
        StatusCode::OK,
    )
    .await;
    let before = s.unbilled(&today).await;
    assert!(
        before
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["expenses"] == "0"),
        "{before}"
    );
    s.approve(report["id"].as_str().unwrap()).await;
    // Approved: the hotel, marked up, is billable; the dinner isn't.
    let after = s.unbilled(&today).await;
    let acme = &after[0];
    assert_eq!(acme["expenses"], "198");
    let line = acme["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "expense")
        .unwrap();
    assert!(
        line["description"]
            .as_str()
            .unwrap()
            .starts_with("Travel and lodging: Hotel")
    );
    let run = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/billing/runs",
            json!({ "orgId": s.provider, "from": today, "to": today, "clients": [s.acme] }),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(run["invoices"], 1);
    // Billed once.
    let again = s.unbilled(&today).await;
    assert!(
        again
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["expenses"] == "0"),
        "{again}"
    );
}

#[tokio::test]
async fn approved_reports_are_exported_and_paid_back() {
    let Some(s) = setup().await else { return };
    let (_, tech_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech_email).await;
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let parts = s.category("Parts and supplies").await;
    let what = unique("=Cable");
    let e = s
        .ok(&tech, "POST", "/api/v1/expenses",
            json!({ "categoryId": parts, "spentOn": today, "description": what, "amount": "12.50", "orgId": s.acme }),
            StatusCode::CREATED)
        .await;
    let r = s
        .ok(
            &tech,
            "POST",
            "/api/v1/expense-reports",
            json!({ "entryIds": [e["id"]] }),
            StatusCode::CREATED,
        )
        .await;
    let id = r["id"].as_str().unwrap();
    // Not until it's approved; not by the technician.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/expense-reports/{id}/reimbursed"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    s.approve(id).await;
    let (status, _) = s
        .call(
            &tech,
            "POST",
            &format!("/api/v1/expense-reports/{id}/reimbursed"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Without an accounting system, sending it says so.
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/expense-reports/{id}/sync"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // The export has it, its formula-looking text defused.
    let req = Request::get("/api/v1/expense-reports/export.csv")
        .header("x-requested-with", "huxley")
        .header("cookie", &s.owner)
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csv =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(csv.contains(&format!("\"'{what}\"")), "{csv}");
    let paid = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/expense-reports/{id}/reimbursed"),
            json!(null),
            StatusCode::OK,
        )
        .await;
    assert_eq!(paid["status"], "reimbursed");
    let entry = s
        .ok(
            &tech,
            "GET",
            &format!("/api/v1/expenses/{}", e["id"].as_str().unwrap()),
            json!(null),
            StatusCode::OK,
        )
        .await;
    assert_eq!(entry["status"], "reimbursed");
}
