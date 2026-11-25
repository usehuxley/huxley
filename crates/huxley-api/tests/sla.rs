//! SLAs against a real PostgreSQL (ADR-037): policies and where they
//! apply, each ticket's clocks (due times, pausing, re-targeting, meeting
//! them), and the sweep that marks warnings and breaches.

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

const PASSWORD: &str = "sla api passphrase";

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
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Sla MSP").await;
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
    let email = format!("{}@example.com", unique("desk").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Desk User", PASSWORD)
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

    async fn ok(&self, cookie: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
        let (status, value) = self.call(cookie, method, uri, body).await;
        assert!(status.is_success(), "{method} {uri}: {status} {value}");
        value
    }

    async fn ticket(&self, cookie: &str, body: Value) -> Value {
        let (status, ticket) = self
            .call(cookie, "POST", "/api/v1/tickets", Some(body))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{ticket}");
        ticket
    }
}

fn field(problem: &Value) -> String {
    problem["errors"][0]["pointer"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn url(ticket: &Value) -> String {
    format!("/api/v1/tickets/{}", ticket["id"].as_str().unwrap())
}

fn when(value: &Value) -> i64 {
    time::OffsetDateTime::parse(
        value
            .as_str()
            .unwrap_or_else(|| panic!("not a time: {value}")),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp()
}

/// A round-the-clock policy assigned to `org`: P1 60/120, P2 30/120 minutes.
async fn always_policy(s: &Setup, org: Uuid) -> Value {
    let policy = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/servicedesk/sla-policies",
            Some(json!({
            "name": unique("Always"), "hoursSource": "always",
            "targets": { "p1": { "response": 15, "resolution": 60 },
                         "p2": { "response": 30, "resolution": 120 } } })),
        )
        .await;
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/orgs/{org}/sla"),
        Some(json!({ "policyId": policy["id"] })),
    )
    .await;
    policy
}

async fn status(s: &Setup, t: &Value, to: &str, extra: Value) -> Value {
    let mut body = json!({ "status": to, "version": t["version"] });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    s.ok(&s.owner, "POST", &format!("{}/status", url(t)), Some(body))
        .await
}

#[tokio::test]
async fn clocks_run_pause_and_stop() {
    let Some(s) = setup().await else { return };
    let policy = always_policy(&s, s.acme).await;
    // Impact 2, urgency 1: P2.
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Clocks", "urgency": 1 }),
        )
        .await;
    let created = when(&t["createdAt"]);
    assert_eq!(t["slaPolicy"], policy["name"]);
    assert!(
        (when(&t["responseDueAt"]) - (created + 30 * 60)).abs() <= 2,
        "{t}"
    );
    assert!(
        (when(&t["resolutionDueAt"]) - (created + 120 * 60)).abs() <= 2,
        "{t}"
    );
    assert_eq!(
        (t["responseMet"].clone(), t["slaPaused"].clone()),
        (Value::Null, json!(false))
    );

    // Re-targeted by priority: P1's resolution is 60 minutes.
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url(&t),
            Some(json!({ "version": t["version"], "impact": 1 })),
        )
        .await;
    assert_eq!(t["priority"], "p1");
    assert!(
        (when(&t["resolutionDueAt"]) - (created + 60 * 60)).abs() <= 3,
        "{t}"
    );

    // Waiting on the client pauses resolution (not response).
    let t = status(&s, &t, "waiting_client", json!({})).await;
    assert_eq!(t["slaPaused"], true);
    assert!(t["resolutionDueAt"].is_null());
    assert!(t["responseDueAt"].is_string());
    let t = status(&s, &t, "in_progress", json!({})).await;
    assert_eq!(t["slaPaused"], false);
    assert!(when(&t["resolutionDueAt"]) >= created + 60 * 60 - 3, "{t}");

    // A reply meets the response target; resolving meets resolution.
    s.ok(
        &s.owner,
        "POST",
        &format!("{}/notes", url(&t)),
        Some(json!({ "kind": "public", "body": "Hi" })),
    )
    .await;
    let t = s.ok(&s.owner, "GET", &url(&t), None).await;
    assert_eq!(t["responseMet"], true);
    let t = status(&s, &t, "resolved", json!({ "resolution": "Done." })).await;
    assert_eq!(
        (t["resolutionMet"].clone(), t["resolutionDueAt"].clone()),
        (json!(true), Value::Null)
    );
    assert_eq!(t["slaPaused"], false);
    // Reopened: running again, not yet met.
    let t = status(&s, &t, "in_progress", json!({})).await;
    assert!(
        t["resolutionMet"].is_null() && t["resolutionDueAt"].is_string(),
        "{t}"
    );
}

#[tokio::test]
async fn policies_follow_the_organization_tree() {
    let Some(s) = setup().await else { return };
    let url = |org: Uuid| format!("/api/v1/orgs/{org}/sla");
    // The seeded default, until something is set.
    let a = s.ok(&s.owner, "GET", &url(s.globex), None).await;
    assert_eq!(
        (a["source"].as_str(), a["policyId"].clone()),
        (Some("default"), Value::Null)
    );

    // Set at the provider: inherited below, overridden at Acme.
    let above = always_policy(&s, s.provider).await;
    let a = s.ok(&s.owner, "GET", &url(s.globex), None).await;
    assert_eq!(
        (a["source"].as_str(), a["effectivePolicyId"].clone()),
        (Some("above"), above["id"].clone())
    );
    let here = always_policy(&s, s.acme).await;
    let a = s.ok(&s.owner, "GET", &url(s.acme), None).await;
    assert_eq!(
        (a["source"].as_str(), a["effectivePolicyId"].clone()),
        (Some("here"), here["id"].clone())
    );

    // Archiving Acme's falls back to the provider's; tickets already open keep theirs.
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Kept" }))
        .await;
    let id = here["id"].as_str().unwrap();
    let path = format!("/api/v1/servicedesk/sla-policies/{id}");
    assert_eq!(
        s.call(&s.owner, "DELETE", &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    let a = s.ok(&s.owner, "GET", &url(s.acme), None).await;
    assert_eq!(a["effectivePolicyId"], above["id"]);
    assert_eq!(
        s.ok(&s.owner, "GET", &self::url(&t), None).await["slaPolicy"],
        here["name"]
    );

    // Clearing the provider's: back to the default.
    s.ok(
        &s.owner,
        "PUT",
        &url(s.provider),
        Some(json!({ "policyId": null })),
    )
    .await;
    assert_eq!(
        s.ok(&s.owner, "GET", &url(s.globex), None).await["source"],
        "default"
    );
}

#[tokio::test]
async fn business_hours_come_from_the_clients_site() {
    let Some(s) = setup().await else { return };
    // Globex follows the default (client's site hours); give it a site.
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/orgs/{}/sla", s.provider),
        Some(json!({ "policyId": null })),
    )
    .await;
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/orgs/{}/sites", s.globex),
        Some(json!({
        "name": "HQ", "timezone": "America/New_York", "hours": { "mon": [["08:00", "18:00"]] } })),
    )
    .await;
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.globex, "title": "Hours" }))
        .await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let calendar: Value =
        sqlx::query_scalar("SELECT calendar FROM sd_ticket_sla WHERE ticket_id = $1")
            .bind(Uuid::parse_str(t["id"].as_str().unwrap()).unwrap())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        calendar,
        json!({ "kind": "weekly", "tz": "America/New_York", "hours": { "mon": [["08:00", "18:00"]] } })
    );
    // Due within Monday opening hours in New York.
    let due = when(&t["responseDueAt"]);
    let local = jiff::Timestamp::from_second(due)
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::get("America/New_York").unwrap());
    assert_eq!(local.weekday(), jiff::civil::Weekday::Monday, "{local}");
    assert!((8..=18).contains(&local.hour()), "{local}");
}

#[tokio::test]
async fn the_sweep_marks_warnings_and_breaches_once() {
    let Some(s) = setup().await else { return };
    always_policy(&s, s.acme).await;
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": unique("Late") }),
        )
        .await;
    let id = Uuid::parse_str(t["id"].as_str().unwrap()).unwrap();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    // As if the response were long overdue.
    sqlx::query(
        "UPDATE sd_ticket_sla SET response_warn_at = now() - interval '2 hours', \
                response_due_at = now() - interval '1 hour' WHERE ticket_id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .unwrap();
    huxley_servicedesk::sla::sweep(&mut tx).await.unwrap();
    huxley_servicedesk::sla::sweep(&mut tx).await.unwrap();
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events_outbox WHERE subject_id = $1 AND event_type LIKE 'ticket.sla%' \
          ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(events, ["ticket.sla_warning", "ticket.sla_breached"]);
    let t = s.ok(&s.owner, "GET", &url(&t), None).await;
    assert!(t["responseBreachedAt"].is_string());
    let activity = s
        .ok(&s.owner, "GET", &format!("{}/activity", url(&t)), None)
        .await;
    let alerts: Vec<&Value> = activity
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "sla")
        .collect();
    assert_eq!(alerts.len(), 2);
    assert_eq!(
        alerts[1]["details"],
        json!({ "clock": "response", "alert": "breached" })
    );
}

#[tokio::test]
async fn policies_are_checked_and_configured_by_admins() {
    let Some(s) = setup().await else { return };
    let base = "/api/v1/servicedesk/sla-policies";
    for (body, pointer) in [
        (json!({ "name": " " }), "/name"),
        (
            json!({ "name": "x", "hoursSource": "lunar" }),
            "/hoursSource",
        ),
        (
            json!({ "name": "x", "timezone": "Mars/Olympus" }),
            "/timezone",
        ),
        (json!({ "name": "x", "hours": { "funday": [] } }), "/hours"),
        (
            json!({ "name": "x", "targets": { "p9": { "response": 5 } } }),
            "/targets",
        ),
        (
            json!({ "name": "x", "targets": { "p1": { "response": 0 } } }),
            "/targets",
        ),
    ] {
        let (status, problem) = s.call(&s.owner, "POST", base, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} {problem}");
        assert_eq!(field(&problem), pointer, "{body}");
    }
    // The default can't be archived or un-defaulted.
    let list = s.ok(&s.owner, "GET", base, None).await;
    let default = list
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["isDefault"] == true)
        .unwrap()
        .clone();
    let id = default["id"].as_str().unwrap();
    let (status, _) = s
        .call(&s.owner, "DELETE", &format!("{base}/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &format!("{base}/{id}"),
            Some(json!({ "name": default["name"], "version": default["version"] })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Technicians see policies but can't change them or assign them.
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.acme))).await.1,
    )
    .await;
    s.ok(&tech, "GET", base, None).await;
    assert!(
        s.ok(&tech, "GET", &format!("/api/v1/orgs/{}/sla", s.acme), None)
            .await
            .is_object()
    );
    let (status, _) = s
        .call(&tech, "POST", base, Some(json!({ "name": unique("Mine") })))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .call(
            &tech,
            "PUT",
            &format!("/api/v1/orgs/{}/sla", s.acme),
            Some(json!({ "policyId": null })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn holidays_stop_the_clock() {
    let Some(s) = setup().await else { return };
    let today = jiff::Zoned::now()
        .with_time_zone(jiff::tz::TimeZone::UTC)
        .date();
    // A week of holidays from today, and one far off.
    let mut holidays: Vec<Value> = (0..7)
        .map(|d| json!({ "date": today.saturating_add(jiff::Span::new().days(d)).to_string(), "name": "Shutdown" }))
        .collect();
    holidays.push(json!({ "date": "2099-12-25", "name": "Far off" }));
    let all_day = json!([["00:00", "24:00"]]);
    let policy = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/servicedesk/sla-policies",
            Some(json!({
            "name": unique("Holidays"), "hoursSource": "policy", "timezone": "UTC",
            "hours": { "mon": all_day, "tue": all_day, "wed": all_day, "thu": all_day,
                       "fri": all_day, "sat": all_day, "sun": all_day },
            "holidays": holidays,
            "targets": { "p3": { "response": 60 } } })),
        )
        .await;
    assert_eq!(policy["holidays"].as_array().unwrap().len(), 8);
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/orgs/{}/sla", s.acme),
        Some(json!({ "policyId": policy["id"] })),
    )
    .await;
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Over the shutdown" }),
        )
        .await;
    // Due an hour into the first day after the week off.
    let back = today.saturating_add(jiff::Span::new().days(7));
    let due = jiff::Timestamp::from_second(when(&t["responseDueAt"]))
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC);
    assert_eq!(
        (due.date(), due.hour(), due.minute()),
        (back, 1, 0),
        "{due}"
    );
    // The ticket keeps the holidays within two years; the far one isn't.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let calendar: Value =
        sqlx::query_scalar("SELECT calendar FROM sd_ticket_sla WHERE ticket_id = $1")
            .bind(Uuid::parse_str(t["id"].as_str().unwrap()).unwrap())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    let closed = calendar["closed"].as_array().unwrap();
    assert_eq!(closed.len(), 7, "{calendar}");
    assert!(!closed.contains(&json!("2099-12-25")));
    // Dates are checked; duplicates are folded.
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/servicedesk/sla-policies",
            Some(json!({
            "name": unique("Bad"), "holidays": [{ "date": "25/12/2026" }] })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/holidays".into())
    );
    let folded = s
        .ok(&s.owner, "POST", "/api/v1/servicedesk/sla-policies", Some(json!({
            "name": unique("Folded"), "holidays": [{ "date": "2026-12-26" }, { "date": "2026-12-25", "name": "Christmas" }, { "date": "2026-12-26", "name": "Boxing Day" }] })))
        .await;
    assert_eq!(
        folded["holidays"],
        json!([{ "date": "2026-12-25", "name": "Christmas" }, { "date": "2026-12-26", "name": "Boxing Day" }])
    );
}
