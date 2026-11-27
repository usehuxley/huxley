//! Invoicing: billing runs from time, recurring charges and projects; invoice lifecycle.

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
    for module in ["crm", "servicedesk", "contracts", "projects", "billing"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Stock MSP").await;
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
    /// Bills time at this hourly rate by default (USD, 30 days).
    async fn rate(&self, rate: &str) {
        self.ok(
            &self.owner,
            "PUT",
            "/api/v1/billing/settings",
            json!({ "orgId": self.provider, "hourlyRate": rate, "currency": "USD", "paymentDays": 30,
                    "approvedTimeOnly": false }),
            StatusCode::OK,
        )
        .await;
    }

    /// A ticket at the client with time on it (not on any agreement).
    async fn time(&self, worked_on: &str, minutes: i32) -> String {
        let t = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/tickets",
                json!({ "orgId": self.acme, "title": unique("Printer jam") }),
                StatusCode::CREATED,
            )
            .await;
        let ticket = Uuid::parse_str(t["id"].as_str().unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let (user, work_type): (Uuid, Uuid) = sqlx::query_as(
            "SELECT (SELECT id FROM users ORDER BY created_at LIMIT 1), (SELECT id FROM sd_work_types WHERE name = 'Remote support')",
        )
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let entry = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO sd_time_entries (id, user_id, org_id, ticket_id, work_type_id, worked_on, minutes, billable) \
             VALUES ($1, $2, $3, $4, $5, $6::date, $7, true)",
        )
        .bind(entry)
        .bind(user)
        .bind(self.acme)
        .bind(ticket)
        .bind(work_type)
        .bind(worked_on)
        .bind(minutes)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        entry.to_string()
    }

    async fn run(&self, from: &str, to: &str) -> Vec<Value> {
        let run = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/billing/runs",
                json!({ "orgId": self.provider, "from": from, "to": to, "clients": [self.acme] }),
                StatusCode::CREATED,
            )
            .await;
        let (_, list) = self
            .call(
                &self.owner,
                "GET",
                &format!("/api/v1/invoices?runId={}", run["id"].as_str().unwrap()),
                None,
            )
            .await;
        let mut out = Vec::new();
        for i in list.as_array().unwrap() {
            let (_, full) = self
                .call(
                    &self.owner,
                    "GET",
                    &format!("/api/v1/invoices/{}", i["id"].as_str().unwrap()),
                    None,
                )
                .await;
            out.push(full);
        }
        out
    }
}

#[tokio::test]
async fn a_billing_run_drafts_invoices_from_time_recurring_charges_and_projects() {
    let Some(s) = setup().await else { return };
    s.rate("100").await;
    // 90 minutes before the agreement starts: billed at the default rate.
    s.time("2030-01-10", 90).await;
    // A managed agreement starting mid-January: its first month is prorated.
    s.ok(
        &s.owner,
        "POST",
        "/api/v1/agreements",
        json!({ "orgId": s.acme, "agreement": { "name": unique("Managed"), "kind": "managed", "status": "active",
            "startsOn": "2030-01-16", "lines": [{ "description": "Managed workstation", "quantity": "10",
            "unitPrice": "50", "billing": "monthly" }] } }),
        StatusCode::CREATED,
    )
    .await;
    // A completed fixed-fee project, and a milestone project with one reached.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let fixed = Uuid::now_v7();
    let staged = Uuid::now_v7();
    for (id, name, method, status) in [
        (fixed, "Firewall swap", "fixed_fee", "completed"),
        (staged, "Office move", "milestone", "active"),
    ] {
        sqlx::query(
            "INSERT INTO prj_projects (id, org_id, name, start_on, billing_method, status, fee_amount) \
             VALUES ($1, $2, $3, '2030-01-01', $4, $5, 2000)",
        )
        .bind(id)
        .bind(s.acme)
        .bind(unique(name))
        .bind(method)
        .bind(status)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO prj_milestones (id, project_id, org_id, name, position, amount, reached_at) \
         VALUES ($1, $2, $3, 'Cabling done', 0, 750, '2030-01-20T12:00:00Z')",
    )
    .bind(Uuid::now_v7())
    .bind(staged)
    .bind(s.acme)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let invoices = s.run("2030-01-01", "2030-01-31").await;
    assert_eq!(invoices.len(), 1, "{invoices:?}");
    let inv = &invoices[0];
    let line = |kind: &str| {
        inv["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["kind"] == kind)
            .cloned()
            .unwrap()
    };
    let time = line("time");
    assert_eq!(
        (time["quantity"].clone(), time["unitPrice"].clone()),
        (json!("1.5"), json!("100"))
    );
    let recurring = line("recurring");
    // 16 of 31 days: 10 × 0.5161 = 5.161.
    assert_eq!(recurring["quantity"], "5.161");
    assert!(
        recurring["description"]
            .as_str()
            .unwrap()
            .contains("16 of 31 days"),
        "{recurring}"
    );
    let projects: Vec<&Value> = inv["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["kind"] == "project")
        .collect();
    assert_eq!(projects.len(), 2);
    // 150 + 258.05 + 2000 + 750.
    assert_eq!(inv["total"], "3158.05");

    // Billed once: a second run finds nothing; voiding gives it all back.
    assert_eq!(s.run("2030-01-01", "2030-01-31").await.len(), 0);
    let id = inv["id"].as_str().unwrap();
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/invoices/{id}/void"),
        json!({ "version": inv["version"] }),
        StatusCode::OK,
    )
    .await;
    let again = s.run("2030-01-01", "2030-01-31").await;
    assert_eq!(again[0]["total"], "3158.05");
    // February: a full month of the agreement.
    let feb = s.run("2030-02-01", "2030-02-28").await;
    assert_eq!(feb[0]["total"], "500");
}

#[tokio::test]
async fn an_invoice_is_edited_held_approved_sent_and_credited() {
    let Some(s) = setup().await else { return };
    s.rate("120").await;
    s.time("2031-03-03", 60).await;
    let inv = s.run("2031-03-01", "2031-03-31").await.remove(0);
    let id = inv["id"].as_str().unwrap().to_owned();
    let path = |p: &str| format!("/api/v1/invoices/{id}{p}");
    // Edited by hand: the line is flagged, and a manual line is added.
    let first = inv["lines"][0].clone();
    let edited = s
        .ok(
            &s.owner,
            "PUT",
            &path(""),
            json!({ "version": inv["version"], "lines": [
                { "id": first["id"], "description": first["description"], "quantity": "1", "unitPrice": "110" },
                { "description": "Travel", "quantity": "1", "unitPrice": "40" }
            ] }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(edited["lines"][0]["edited"], true);
    assert_eq!(edited["lines"][1]["kind"], "manual");
    assert_eq!(edited["total"], "150");
    // Held: not approvable until released.
    let held = s
        .ok(
            &s.owner,
            "POST",
            &path("/hold"),
            json!({ "onHold": true, "reason": "Check travel" }),
            StatusCode::OK,
        )
        .await;
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &path("/approve"),
            Some(json!({ "version": held["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let free = s
        .ok(
            &s.owner,
            "POST",
            &path("/hold"),
            json!({ "onHold": false }),
            StatusCode::OK,
        )
        .await;
    let approved = s
        .ok(
            &s.owner,
            "POST",
            &path("/approve"),
            // Changed by hand, it needs review; the owner may override.
            json!({ "version": free["version"], "overrideReview": true }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(approved["status"], "approved");
    assert!(approved["dueOn"].is_string());
    let sent = s
        .ok(
            &s.owner,
            "POST",
            &path("/sent"),
            json!(null),
            StatusCode::OK,
        )
        .await;
    assert_eq!(sent["status"], "sent");
    // Locked: corrections are credit memos.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &path(""),
            Some(json!({ "version": sent["version"], "lines": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &path("/void"),
            Some(json!({ "version": sent["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let memo = s
        .ok(
            &s.owner,
            "POST",
            &path("/credit"),
            json!({ "lineIds": [sent["lines"][1]["id"]], "reason": "No travel after all" }),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(memo["kind"], "credit_memo");
    assert_eq!(memo["total"], "-40");
    assert_eq!(memo["creditOf"], json!(id));
}

#[tokio::test]
async fn a_run_is_approved_in_one_pass_and_hand_edits_go_to_review() {
    let Some(s) = setup().await else { return };
    s.rate("100").await;
    s.time("2034-04-04", 60).await;
    let inv = s.run("2034-04-01", "2034-04-30").await.remove(0);
    assert_eq!(inv["reviewStatus"], "not_needed");
    let id = inv["id"].as_str().unwrap().to_owned();
    let run = inv["runId"].as_str().unwrap().to_owned();
    let line = inv["lines"][0].clone();
    let edited = s
        .ok(
            &s.owner,
            "PUT",
            &format!("/api/v1/invoices/{id}"),
            json!({ "version": inv["version"], "lines": [
                { "id": line["id"], "description": line["description"], "quantity": "1", "unitPrice": "75" }
            ] }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(edited["reviewStatus"], "needed");
    // The run's approval flags it and submits it for review.
    let pass = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/billing/runs/{run}/approve"),
            json!({ "submitReviews": true }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(pass["approved"], json!([]));
    assert_eq!(pass["exceptions"][0]["reason"], "submitted for review");
    let (_, review) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/invoices/{id}/review"),
            None,
        )
        .await;
    assert_eq!(review["status"], "in_review");
    assert_eq!(review["canApprove"], false);
    assert_eq!(review["canOverride"], true);
    assert!(
        review["reasons"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("changed by hand")
    );
    // A comment on the line.
    let c = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/invoices/{id}/comments"),
            json!({ "body": "Why 75?", "lineId": line["id"] }),
            StatusCode::CREATED,
        )
        .await;
    assert_eq!(c["lineName"], line["description"]);
    let (_, all) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/invoices/{id}/comments"),
            None,
        )
        .await;
    assert_eq!(all.as_array().unwrap().len(), 1);
    // Without the override, approval waits for the review.
    let now = s
        .call(&s.owner, "GET", &format!("/api/v1/invoices/{id}"), None)
        .await
        .1;
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/invoices/{id}/approve"),
            Some(json!({ "version": now["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_invoice_prints() {
    let Some(s) = setup().await else { return };
    s.rate("90").await;
    s.time("2032-05-05", 30).await;
    let inv = s.run("2032-05-01", "2032-05-31").await.remove(0);
    let id = inv["id"].as_str().unwrap().to_owned();
    let path = |p: &str| format!("/api/v1/invoices/{id}{p}");
    let req = Request::get(path("/pdf"))
        .header("x-requested-with", "huxley")
        .header("cookie", &s.owner)
        .body(Body::empty())
        .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/pdf");
}

#[tokio::test]
async fn project_fees_and_milestone_amounts_are_set_and_billed() {
    let Some(s) = setup().await else { return };
    s.rate("100").await;
    let p = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            json!({ "orgId": s.acme, "name": unique("Phone system"), "startOn": "2033-01-01", "billingMethod": "milestone" }),
            StatusCode::CREATED,
        )
        .await;
    let id = p["id"].as_str().unwrap().to_owned();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let milestone = Uuid::now_v7();
    sqlx::query("INSERT INTO prj_milestones (id, project_id, org_id, name, position) VALUES ($1, $2, $3, 'Go live', 0)")
        .bind(milestone)
        .bind(Uuid::parse_str(&id).unwrap())
        .bind(s.acme)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let after = s
        .ok(
            &s.owner,
            "PUT",
            &format!("/api/v1/projects/{id}/milestones/{milestone}/amount"),
            json!({ "amount": 1200.0 }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(after["milestones"][0]["amount"], 1200.0);
    let fee = s
        .ok(
            &s.owner,
            "PATCH",
            &format!("/api/v1/projects/{id}"),
            json!({ "feeAmount": 5000.0, "version": after["version"] }),
            StatusCode::OK,
        )
        .await;
    assert_eq!(fee["feeAmount"], 5000.0);
    // Not reached yet: nothing to bill; reached: billed.
    assert_eq!(s.run("2033-01-01", "2033-01-31").await.len(), 0);
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/projects/{id}/milestones/{milestone}"),
        json!({ "reached": true }),
        StatusCode::OK,
    )
    .await;
    let today = time::OffsetDateTime::now_utc().date();
    let inv = s.run(&today.to_string(), &today.to_string()).await;
    let line = inv[0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "project")
        .cloned()
        .unwrap();
    assert_eq!(line["unitPrice"], "1200");
}
