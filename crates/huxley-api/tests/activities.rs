//! Sales activities: tasks, calls, notes, and email logged from a sales mailbox.

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

const PASSWORD: &str = "activities api passphrase";

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
    for module in ["crm", "servicedesk", "contracts", "sales"] {
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
}

impl Setup {
    async fn opportunity(&self) -> String {
        let (status, o) = self
            .call(
                &self.owner,
                "POST",
                "/api/v1/opportunities",
                Some(json!({ "orgId": self.acme, "opportunity": { "name": unique("Backup") } })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{o}");
        o["id"].as_str().unwrap().to_owned()
    }

    async fn activity(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        self.call(cookie, "POST", "/api/v1/activities", Some(body))
            .await
    }
}

#[tokio::test]
async fn tasks_calls_and_notes_are_kept_on_a_deal() {
    let Some(s) = setup().await else { return };
    let (rep_id, rep) = user(&s.pools, Some(("sales", s.provider))).await;
    let rep = login(&s.app, &rep).await;
    let opp = s.opportunity().await;

    // A task for the rep, from the owner: they're told.
    let (status, task) = s
        .activity(
            &s.owner,
            json!({ "kind": "task", "subject": "Send the proposal", "opportunityId": opp,
                "ownerId": rep_id, "dueAt": "2027-01-05T09:00:00Z" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    assert_eq!(task["orgId"], json!(s.acme));
    assert!(task["doneAt"].is_null());
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'sales.task'",
    )
    .bind(rep_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(told, 1);

    // A call that happened, and a note (done as written).
    let (_, call) = s
        .activity(
            &rep,
            json!({ "kind": "call", "subject": "Discovery call", "opportunityId": opp,
            "durationMinutes": 30, "outcome": "connected", "done": true }),
        )
        .await;
    assert!(call["doneAt"].is_string());
    let (_, note) = s
        .activity(
            &rep,
            json!({ "kind": "note", "subject": "They use Veeam today", "opportunityId": opp }),
        )
        .await;
    assert!(note["doneAt"].is_string());
    // Emails come from a sales mailbox, not by hand.
    let (status, _) = s
        .activity(
            &rep,
            json!({ "kind": "email", "subject": "x", "opportunityId": opp }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // It must be about something.
    let (status, _) = s
        .activity(&rep, json!({ "kind": "note", "subject": "Floating" }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (_, mine) = s
        .call(&rep, "GET", "/api/v1/activities?mine=true&open=true", None)
        .await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["subject"], "Send the proposal");
    let id = task["id"].as_str().unwrap();
    let (status, done) = s
        .call(
            &rep,
            "POST",
            &format!("/api/v1/activities/{id}/done"),
            Some(json!({ "done": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (_, mine) = s
        .call(&rep, "GET", "/api/v1/activities?mine=true&open=true", None)
        .await;
    assert_eq!(mine, json!([]));
    let (_, all) = s
        .call(
            &rep,
            "GET",
            &format!("/api/v1/activities?opportunityId={opp}"),
            None,
        )
        .await;
    assert_eq!(all.as_array().unwrap().len(), 3);

    // Technicians don't see sales activities.
    let (_, tech) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &tech).await;
    let (status, _) = s
        .call(&tech, "GET", &format!("/api/v1/activities/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .call(&rep, "DELETE", &format!("/api/v1/activities/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn email_to_a_sales_mailbox_is_logged_not_ticketed() {
    let Some(s) = setup().await else { return };
    let opp = s.opportunity().await;
    let who = format!("{}@acme.example", unique("pat").replace(' ', "."));
    let (status, contact) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.acme),
            Some(json!({ "firstName": "Pat", "lastName": "Lee", "email": who })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{contact}");
    let address = format!("{}@msp.example", unique("sales-log").replace(' ', "-"));
    let (status, mailbox) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/mailboxes",
            Some(json!({ "orgId": s.provider, "name": unique("Sales BCC"), "address": address, "purpose": "sales_log" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{mailbox}");
    assert_eq!(mailbox["purpose"], "sales_log");
    let raw = format!(
        "From: Sam <sam@msp.example>\r\nTo: Pat Lee <{who}>\r\nBcc: {address}\r\nSubject: Your backup quote\r\n\
         Message-ID: <{}@msp.example>\r\n\r\nHi Pat, the quote is attached.\r\n",
        Uuid::now_v7()
    );
    let req = Request::post(format!(
        "/api/v1/mailboxes/{}/messages",
        mailbox["id"].as_str().unwrap()
    ))
    .header("x-requested-with", "huxley")
    .header("cookie", &s.owner)
    .header("content-type", "message/rfc822")
    .body(Body::from(raw))
    .unwrap();
    let res = s.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let email: Value = serde_json::from_slice(&bytes).unwrap();
    let email = Uuid::parse_str(email["emailId"].as_str().unwrap()).unwrap();

    // The ticket workflow passes it by; the logging step files it.
    let ctx = RequestContext::system(Via::Workflow);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let outcome = huxley_servicedesk::mail::to_ticket(&mut tx, &ctx, email, s.provider)
        .await
        .unwrap();
    assert_eq!(outcome.outcome, "ignored_sales_log");
    assert!(outcome.ticket_id.is_none());
    tx.rollback().await.unwrap();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let logged = huxley_sales::activities::log_email(&mut tx, &ctx, email)
        .await
        .unwrap();
    assert_eq!(logged.logged, 1);
    assert_eq!(logged.unmatched, vec!["sam@msp.example".to_owned()]);
    // Logged once, however often it's run.
    assert_eq!(
        huxley_sales::activities::log_email(&mut tx, &ctx, email)
            .await
            .unwrap()
            .logged,
        0
    );
    tx.commit().await.unwrap();

    let (_, on_deal) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/activities?opportunityId={opp}"),
            None,
        )
        .await;
    let e = &on_deal[0];
    assert_eq!(e["kind"], "email");
    assert_eq!(e["direction"], "out");
    assert_eq!(e["subject"], "Your backup quote");
    assert_eq!(e["contactName"], "Pat Lee");
}
