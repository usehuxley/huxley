//! Timesheets through the API (design doc §8.15), on its own database: a
//! technician's week is submitted, rejected with a comment, corrected,
//! approved and locked; only approved time reaches a billing preview;
//! reopening needs a reason; payroll exports approved time.

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
use time::{Duration, OffsetDateTime, Weekday};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "timesheet api passphrase";

struct Setup {
    app: axum::Router,
    owner: String,
    tech: String,
    client: Uuid,
}

async fn user(pools: &TestPools, email: &str) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, email, email, PASSWORD)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    u.id
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

async fn setup(pools: &TestPools) -> Setup {
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let app = router(AppState::new(pools.app.clone(), auth, crypto(), config));
    let root = ensure_root(pools).await;
    let owner_id = user(pools, "lead@example.com").await;
    let tech_id = user(pools, "tech@example.com").await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_authz::grant_root_owner(&mut tx, &ctx, owner_id)
        .await
        .unwrap();
    for module in ["crm", "servicedesk", "workforce", "contracts"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let owner = huxley_authz::load_principal(&mut tx, owner_id)
        .await
        .unwrap();
    let technician: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE key = 'technician' AND is_system")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    huxley_authz::assign(&mut tx, &ctx, &owner, tech_id, technician, root, true)
        .await
        .unwrap();
    // The owner leads the technician's team.
    let team = huxley_authz::teams::create(&mut tx, &ctx, "Desk", "")
        .await
        .unwrap();
    huxley_authz::teams::add_member(&mut tx, &ctx, team.id, owner_id, Some(true))
        .await
        .unwrap();
    huxley_authz::teams::add_member(&mut tx, &ctx, team.id, tech_id, None)
        .await
        .unwrap();
    let client = huxley_orgs::create(&mut tx, &ctx, root, huxley_orgs::OrgKind::Client, "Acme")
        .await
        .unwrap()
        .id;
    tx.commit().await.unwrap();
    let owner = login(&app, "lead@example.com").await;
    let tech = login(&app, "tech@example.com").await;
    Setup {
        app,
        owner,
        tech,
        client,
    }
}

impl Setup {
    async fn call(&self, who: &str, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-requested-with", "huxley")
            .header("cookie", who)
            .header("content-type", "application/json")
            .body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
        )
    }

    async fn log(
        &self,
        who: &str,
        ticket: &str,
        work_type: &str,
        day: &str,
        minutes: i32,
    ) -> (StatusCode, Value) {
        self.call(
            who,
            "POST",
            &format!("/api/v1/tickets/{ticket}/time"),
            json!({ "workTypeId": work_type, "workedOn": day, "minutes": minutes, "billable": true }),
        )
        .await
    }
}

/// A T&M agreement at the client, a ticket there, and the remote work type.
async fn fixtures(s: &Setup) -> (String, String, String) {
    let (_, agreement) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/agreements",
            json!({ "orgId": s.client, "agreement": {
                "name": "T&M", "kind": "time_materials", "status": "active",
                "startsOn": "2020-01-01", "rates": [{ "hourlyRate": "100" }],
            } }),
        )
        .await;
    let agreement = agreement["id"].as_str().unwrap().to_owned();
    let (status, ticket) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            json!({ "orgId": s.client, "title": "Email" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ticket}");
    let ticket = ticket["id"].as_str().unwrap().to_owned();
    let (_, types) = s
        .call(&s.tech, "GET", "/api/v1/work-types", Value::Null)
        .await;
    let remote = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Remote support")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (agreement, ticket, remote)
}

#[allow(clippy::needless_pass_by_value)]
fn decision(approve: bool, note: Value) -> Value {
    json!({ "approve": approve, "note": note })
}

/// Last week's Monday.
fn last_monday() -> time::Date {
    let today = OffsetDateTime::now_utc().date();
    let mut d = today - Duration::days(7);
    while d.weekday() != Weekday::Monday {
        d -= Duration::days(1);
    }
    d
}

#[tokio::test]
async fn a_week_is_submitted_rejected_corrected_approved_and_billed() {
    let Some(db) = isolated("timesheets").await else {
        return;
    };
    let s = setup(&db.pools).await;
    let monday = last_monday().to_string();
    let tuesday = (last_monday() + Duration::days(1)).to_string();
    let sunday = (last_monday() + Duration::days(6)).to_string();

    let (agreement, ticket, remote) = fixtures(&s).await;
    assert_eq!(
        s.log(&s.tech, &ticket, &remote, &monday, 60).await.0,
        StatusCode::CREATED
    );
    // The lead's own time, never approved.
    assert_eq!(
        s.log(&s.owner, &ticket, &remote, &tuesday, 30).await.0,
        StatusCode::CREATED
    );

    // Open, then submitted: the week locks.
    let (_, mine) = s
        .call(
            &s.tech,
            "GET",
            &format!("/api/v1/timesheets/mine?date={tuesday}"),
            Value::Null,
        )
        .await;
    assert_eq!(
        (&mine["status"], &mine["periodStart"], &mine["periodEnd"]),
        (&json!("open"), &json!(monday), &json!(sunday))
    );
    assert_eq!(
        (&mine["totalMinutes"], &mine["missingNotes"]),
        (&json!(60), &json!(1))
    );
    let submit = json!({ "periodStart": monday, "note": "All done" });
    let (status, sheet) = s
        .call(&s.tech, "POST", "/api/v1/timesheets/submit", submit.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{sheet}");
    let id = sheet["id"].as_str().unwrap().to_owned();
    let (status, locked) = s.log(&s.tech, &ticket, &remote, &tuesday, 15).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{locked}");
    assert!(locked.to_string().contains("submitted"), "{locked}");
    not_their_own(&s, &id).await;
    // The lead sees it, and rejects it with a comment.
    let (_, queue) = s
        .call(&s.owner, "GET", "/api/v1/timesheets", Value::Null)
        .await;
    assert_eq!(queue.as_array().unwrap().len(), 1, "{queue}");
    assert_eq!(queue[0]["totalMinutes"], 60);
    let decide = format!("/api/v1/timesheets/{id}/decision");
    let (status, _) = s
        .call(&s.owner, "POST", &decide, decision(false, Value::Null))
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a rejection needs a comment"
    );
    let (_, rejected) = s
        .call(
            &s.owner,
            "POST",
            &decide,
            decision(false, json!("Tuesday's call is missing")),
        )
        .await;
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["history"][1]["note"], "Tuesday's call is missing");

    // Corrected and resubmitted; approved; locked.
    assert_eq!(
        s.log(&s.tech, &ticket, &remote, &tuesday, 30).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        s.call(&s.tech, "POST", "/api/v1/timesheets/submit", submit.clone())
            .await
            .0,
        StatusCode::OK
    );
    let (_, approved) = s
        .call(&s.owner, "POST", &decide, decision(true, Value::Null))
        .await;
    assert_eq!(
        (&approved["status"], &approved["totalMinutes"]),
        (&json!("approved"), &json!(90))
    );
    let entry = approved["entries"][0]["id"].as_str().unwrap().to_owned();
    let (status, _) = s
        .call(
            &s.tech,
            "DELETE",
            &format!("/api/v1/time-entries/{entry}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    billing_preview(&s, &agreement, &monday, &sunday).await;

    reopening_and_payroll(&s, &id, &submit, &decide, &monday, &sunday).await;
}

/// Nobody decides their own timesheet.
async fn not_their_own(s: &Setup, id: &str) {
    let (status, _) = s
        .call(
            &s.tech,
            "POST",
            &format!("/api/v1/timesheets/{id}/decision"),
            decision(true, Value::Null),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Only approved time reaches the billing preview.
async fn billing_preview(s: &Setup, agreement: &str, monday: &str, sunday: &str) {
    let usage = |approved_only: bool| {
        format!(
            "/api/v1/agreements/{agreement}/usage?from={monday}&to={sunday}&approvedOnly={approved_only}"
        )
    };
    let (_, all) = s.call(&s.owner, "GET", &usage(false), Value::Null).await;
    assert_eq!(
        (&all["billableMinutes"], &all["amount"]),
        (&json!(120), &json!("200")),
        "{all}"
    );
    let (_, billed) = s.call(&s.owner, "GET", &usage(true), Value::Null).await;
    assert_eq!(
        (&billed["billableMinutes"], &billed["amount"]),
        (&json!(90), &json!("150")),
        "{billed}"
    );
}

async fn reopening_and_payroll(
    s: &Setup,
    id: &str,
    submit: &Value,
    decide: &str,
    monday: &str,
    sunday: &str,
) {
    let reopen = format!("/api/v1/timesheets/{id}/reopen");
    // The technician can't reopen an approved one; the lead needs a reason.
    assert_eq!(
        s.call(&s.tech, "POST", &reopen, json!({})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        s.call(&s.owner, "POST", &reopen, json!({})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (_, open) = s
        .call(&s.owner, "POST", &reopen, json!({ "note": "Wrong client" }))
        .await;
    assert_eq!(open["status"], "open");
    let actions: Vec<&str> = open["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        ["submitted", "rejected", "submitted", "approved", "reopened"]
    );

    // Approved again: payroll has it.
    s.call(&s.tech, "POST", "/api/v1/timesheets/submit", submit.clone())
        .await;
    s.call(&s.owner, "POST", decide, json!({ "approve": true }))
        .await;
    let req = Request::get(format!(
        "/api/v1/timesheets/payroll.csv?from={monday}&to={sunday}"
    ))
    .header("x-requested-with", "huxley")
    .header("cookie", &s.owner)
    .body(Body::empty())
    .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csv =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "person,email,date,kind,minutes,billable");
    assert_eq!(lines.len(), 3, "{csv}");
    assert!(
        lines
            .iter()
            .skip(1)
            .all(|l| l.contains("tech@example.com") && l.contains("Remote support")),
        "{csv}"
    );
    // Not for technicians.
    let (status, _) = s
        .call(
            &s.tech,
            "GET",
            &format!("/api/v1/timesheets/payroll.csv?from={monday}&to={sunday}"),
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
