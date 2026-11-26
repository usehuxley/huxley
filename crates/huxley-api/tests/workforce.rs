//! The `workforce` module's API against a real PostgreSQL (design doc
//! §8.15): the catalogs, people's skills and certifications, requirements,
//! who fits work, and certification expiry events.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "workforce api passphrase";

struct Setup {
    pools: TestPools,
    app: axum::Router,
    provider: Uuid,
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
    Some(setup_with(pools().await?).await)
}

async fn setup_with(pools: TestPools) -> Setup {
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
    for module in ["crm", "servicedesk", "workforce"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Wf MSP").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    Setup {
        pools,
        app,
        provider,
        owner,
    }
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
}

fn field(problem: &Value) -> String {
    problem["errors"][0]["pointer"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The signed-in user's id.
async fn me(s: &Setup, cookie: &str) -> String {
    s.ok(cookie, "GET", "/api/v1/auth/me", None).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn catalogs_are_kept_by_managers() {
    let Some(s) = setup().await else { return };
    let name = unique("Exchange Online");
    let skill = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": name, "category": "Microsoft 365" })),
        )
        .await;
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": name.to_uppercase() })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/name".into())
    );
    let url = format!("/api/v1/workforce/skills/{}", skill["id"].as_str().unwrap());
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &url,
            Some(json!({ "name": "x", "version": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let cert = s
        .ok(&s.owner, "POST", "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "Microsoft", "name": unique("Azure Administrator"), "code": "AZ-104", "validityMonths": 12 })))
        .await;
    assert_eq!(
        (cert["validityMonths"].clone(), cert["holders"].clone()),
        (json!(12), json!(0))
    );
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "X", "name": "Y", "validityMonths": 0 })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/validityMonths".into())
    );

    // A technician at a provider reads, but doesn't keep the catalogs.
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let skills = s.ok(&tech, "GET", "/api/v1/workforce/skills", None).await;
    assert!(
        skills
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["id"] == skill["id"])
    );
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": unique("Mine") })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A skill requirement needs a level.
    let target = format!(
        "/api/v1/workforce/requirements/ticket_type/{}",
        Uuid::now_v7()
    );
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            &target,
            Some(json!([{ "skillId": skill["id"] }])),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/minLevel".into())
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &url, None).await.0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn people_assess_their_skills_and_managers_verify() {
    let Some(s) = setup().await else { return };
    let skill = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": unique("Firewalls") })),
        )
        .await;
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let tech_id = me(&s, &tech).await;
    let other = user(&s.pools, Some(("technician", s.provider))).await.0;
    let skill_url = |who: &str| {
        format!(
            "/api/v1/workforce/people/{who}/skills/{}",
            skill["id"].as_str().unwrap()
        )
    };

    // My own level: self-assessed; I can't verify it, or change someone else's.
    let p = s
        .ok(
            &tech,
            "PUT",
            &skill_url(&tech_id),
            Some(json!({ "level": 3 })),
        )
        .await;
    assert_eq!(
        (
            p["skills"][0]["level"].clone(),
            p["skills"][0]["verifiedBy"].clone()
        ),
        (json!(3), Value::Null)
    );
    let (status, problem) = s
        .call(
            &tech,
            "PUT",
            &skill_url(&tech_id),
            Some(json!({ "level": 4, "verified": true })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/verified".into())
    );
    let (status, _) = s
        .call(
            &tech,
            "PUT",
            &skill_url(&other.to_string()),
            Some(json!({ "level": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, problem) = s
        .call(
            &tech,
            "PUT",
            &skill_url(&tech_id),
            Some(json!({ "level": 5 })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/level".into())
    );

    // A manager verifies it; my next change makes it self-assessed again.
    let p = s
        .ok(
            &s.owner,
            "PUT",
            &skill_url(&tech_id),
            Some(json!({ "level": 3, "verified": true })),
        )
        .await;
    assert_eq!(p["skills"][0]["verifiedBy"], "Desk User");
    assert!(p["skills"][0]["verifiedOn"].is_string());
    let p = s
        .ok(
            &tech,
            "PUT",
            &skill_url(&tech_id),
            Some(json!({ "level": 4 })),
        )
        .await;
    assert_eq!(p["skills"][0]["verifiedBy"], Value::Null);
}

#[tokio::test]
async fn people_keep_their_certifications() {
    let Some(s) = setup().await else { return };
    let cert = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "Cisco", "name": unique("CCNA"), "validityMonths": 36 })),
        )
        .await;
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let tech_id = me(&s, &tech).await;
    // Certifications: expiry from the validity; dates are checked.
    let certs = format!("/api/v1/workforce/people/{tech_id}/certifications");
    let p = s
        .ok(&tech, "POST", &certs, Some(json!({ "certificationId": cert["id"], "earnedOn": "2025-03-01", "credentialId": "CSCO123" })))
        .await;
    let held = p["certifications"][0].clone();
    assert_eq!(
        (
            held["earnedOn"].as_str(),
            held["expiresOn"].as_str(),
            held["state"].as_str()
        ),
        (Some("2025-03-01"), Some("2028-03-01"), Some("valid"))
    );
    for (body, pointer) in [
        (
            json!({ "certificationId": cert["id"], "earnedOn": "March 2025" }),
            "/earnedOn",
        ),
        (
            json!({ "certificationId": cert["id"], "earnedOn": "2025-02-30" }),
            "/earnedOn",
        ),
        (
            json!({ "certificationId": cert["id"], "earnedOn": "2025-03-01", "expiresOn": "2024-01-01" }),
            "/expiresOn",
        ),
        (
            json!({ "certificationId": cert["id"], "earnedOn": "2025-03-01", "verifyUrl": "http://x" }),
            "/verifyUrl",
        ),
        (
            json!({ "certificationId": Uuid::now_v7(), "earnedOn": "2025-03-01" }),
            "/certificationId",
        ),
    ] {
        let (status, problem) = s.call(&tech, "POST", &certs, Some(body.clone())).await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
    // An old one has expired; renewing it makes it valid.
    let p = s
        .ok(
            &tech,
            "PUT",
            &format!("{certs}/{}", held["id"].as_str().unwrap()),
            Some(json!({ "earnedOn": "2020-01-01", "expiresOn": "2021-01-01" })),
        )
        .await;
    assert_eq!(p["certifications"][0]["state"], "expired");
    let summary = s.ok(&tech, "GET", "/api/v1/workforce/people", None).await;
    let mine = summary
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["userId"] == json!(tech_id))
        .unwrap();
    // An expired certification isn't counted as held.
    assert_eq!(
        (mine["skills"].clone(), mine["certifications"].clone()),
        (json!(0), json!(0))
    );
}

#[tokio::test]
async fn requirements_rank_who_fits_the_work() {
    let Some(s) = setup().await else { return };
    let skill = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": unique("Intune") })),
        )
        .await;
    let cert = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "Microsoft", "name": unique("MD-102") })),
        )
        .await;
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Endpoints") })),
        )
        .await;
    let qid = queue["id"].as_str().unwrap();
    let url = format!("/api/v1/workforce/requirements/queue/{qid}");
    let reqs = s
        .ok(
            &s.owner,
            "PUT",
            &url,
            Some(json!([
                { "skillId": skill["id"], "minLevel": 3 },
                { "certificationId": cert["id"] }
            ])),
        )
        .await;
    assert_eq!(reqs.as_array().unwrap().len(), 2);
    assert_eq!(reqs[0]["certificationId"], cert["id"]);

    // Ana: certified, expert; Ben: not certified, working level; Cy: nothing.
    let (ana, ben, cy) = (
        user(&s.pools, Some(("technician", s.provider))).await.0,
        user(&s.pools, Some(("technician", s.provider))).await.0,
        user(&s.pools, Some(("technician", s.provider))).await.0,
    );
    let level = |who: Uuid, n: i64| {
        let s = &s;
        let path = format!(
            "/api/v1/workforce/people/{who}/skills/{}",
            skill["id"].as_str().unwrap()
        );
        async move {
            s.ok(&s.owner, "PUT", &path, Some(json!({ "level": n })))
                .await
        }
    };
    level(ana, 4).await;
    level(ben, 2).await;
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/workforce/people/{ana}/certifications"),
        Some(json!({ "certificationId": cert["id"], "earnedOn": "2026-01-01" })),
    )
    .await;
    let fits = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/workforce/fit?queueId={qid}"),
            None,
        )
        .await;
    let ours: Vec<&Value> = fits
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| [ana, ben, cy].iter().any(|u| f["userId"] == json!(u)))
        .collect();
    assert_eq!(
        ours.iter().map(|f| f["userId"].clone()).collect::<Vec<_>>(),
        [json!(ana), json!(ben), json!(cy)]
    );
    assert_eq!(
        (ours[0]["qualified"].clone(), ours[0]["skillMatch"].clone()),
        (json!(true), json!(1.0))
    );
    assert_eq!(ours[1]["qualified"], false);
    assert!((ours[1]["skillMatch"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(ours[2]["missing"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn expiring_and_expired_certifications_are_announced_once() {
    let Some(s) = setup().await else { return };
    let cert = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "CompTIA", "name": unique("Security+") })),
        )
        .await;
    let who = user(&s.pools, Some(("technician", s.provider))).await.0;
    let soon = (time::OffsetDateTime::now_utc() + time::Duration::days(10))
        .date()
        .to_string();
    let p = s
        .ok(&s.owner, "POST", &format!("/api/v1/workforce/people/{who}/certifications"),
            Some(json!({ "certificationId": cert["id"], "earnedOn": "2023-01-01", "expiresOn": soon })))
        .await;
    assert_eq!(p["certifications"][0]["state"], "expiring");
    let held = Uuid::parse_str(p["certifications"][0]["id"].as_str().unwrap()).unwrap();

    let events = async |pools: &TestPools| -> Vec<String> {
        let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
        huxley_workforce::requirements::sweep(&mut tx)
            .await
            .unwrap();
        let found = sqlx::query_scalar(
            "SELECT event_type FROM events_outbox WHERE payload->>'held_id' = $1 ORDER BY occurred_at, id",
        )
        .bind(held.to_string())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        found
    };
    assert_eq!(events(&s.pools).await, ["certification.expiring"]);
    assert_eq!(events(&s.pools).await, ["certification.expiring"]);
    // Past its expiry: announced as expired, once.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("UPDATE wf_user_certifications SET expires_on = current_date - 1 WHERE id = $1")
        .bind(held)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        events(&s.pools).await,
        ["certification.expiring", "certification.expired"]
    );
    assert_eq!(events(&s.pools).await.len(), 2);
}

#[tokio::test]
async fn schedules_are_checked_and_kept_by_their_owner() {
    let Some(s) = setup().await else { return };
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let me_id = me(&s, &tech).await;
    let url = format!("/api/v1/workforce/people/{me_id}/schedule");
    assert_eq!(s.ok(&tech, "GET", &url, None).await, Value::Null);
    let set = s
        .ok(
            &tech,
            "PUT",
            &url,
            Some(json!({ "timezone": "Europe/London", "hours": { "mon": [["08:00", "16:00"]] } })),
        )
        .await;
    assert_eq!(set["hours"], json!({ "mon": [["08:00", "16:00"]] }));
    for (body, pointer) in [
        (
            json!({ "timezone": "Mars/Olympus", "hours": { "mon": [["08:00", "16:00"]] } }),
            "/timezone",
        ),
        (json!({ "timezone": "UTC", "hours": {} }), "/hours"),
    ] {
        let (status, problem) = s.call(&tech, "PUT", &url, Some(body.clone())).await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
    // Someone else's: a manager's to set.
    let other = user(&s.pools, Some(("technician", s.provider))).await.0;
    let theirs = format!("/api/v1/workforce/people/{other}/schedule");
    let (status, _) = s
        .call(
            &tech,
            "PUT",
            &theirs,
            Some(json!({ "timezone": "UTC", "hours": { "tue": [["09:00", "17:00"]] } })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    s.ok(
        &s.owner,
        "PUT",
        &theirs,
        Some(json!({ "timezone": "UTC", "hours": { "tue": [["09:00", "17:00"]] } })),
    )
    .await;
    assert_eq!(
        s.ok(&tech, "PUT", &url, Some(Value::Null)).await,
        Value::Null
    );
}

#[tokio::test]
async fn time_off_requests_are_checked() {
    let Some(s) = setup().await else { return };
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let me_id = me(&s, &tech).await;
    let ask = format!("/api/v1/workforce/people/{me_id}/time-off");
    for (body, pointer) in [
        (
            json!({ "startsOn": "2026-12-24", "endsOn": "2026-12-20", "kind": "vacation" }),
            "/endsOn",
        ),
        (
            json!({ "startsOn": "2026-12-24", "endsOn": "2026-12-31", "kind": "nap" }),
            "/kind",
        ),
        (
            json!({ "startsOn": "24/12/2026", "endsOn": "2026-12-31", "kind": "vacation" }),
            "/startsOn",
        ),
    ] {
        let (status, problem) = s.call(&tech, "POST", &ask, Some(body.clone())).await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
}

#[tokio::test]
async fn time_off_is_requested_decided_and_cancelled() {
    let Some(s) = setup().await else { return };
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let me_id = me(&s, &tech).await;
    let ask = format!("/api/v1/workforce/people/{me_id}/time-off");
    let t = s
        .ok(&tech, "POST", &ask, Some(json!({ "startsOn": "2099-12-24", "endsOn": "2099-12-31", "kind": "vacation", "note": "Family" })))
        .await;
    assert_eq!(t["status"], "requested");
    let id = t["id"].as_str().unwrap();
    // I can't decide my own; a manager can, once.
    let (status, _) = s
        .call(
            &tech,
            "POST",
            &format!("/api/v1/workforce/time-off/{id}/decision"),
            Some(json!({ "approve": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let t = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/workforce/time-off/{id}/decision"),
            Some(json!({ "approve": true, "note": "Enjoy" })),
        )
        .await;
    assert_eq!(
        (t["status"].as_str(), t["decidedBy"].as_str()),
        (Some("approved"), Some("Desk User"))
    );
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("/api/v1/workforce/time-off/{id}/decision"),
            Some(json!({ "approve": false })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Mine to cancel; then it's gone from the approved list.
    let t = s
        .ok(
            &tech,
            "POST",
            &format!("/api/v1/workforce/time-off/{id}/cancel"),
            None,
        )
        .await;
    assert_eq!(t["status"], "cancelled");
    let listed = s
        .ok(
            &tech,
            "GET",
            &format!("/api/v1/workforce/time-off?userId={me_id}&status=approved"),
            None,
        )
        .await;
    assert_eq!(listed, json!([]));

    // A manager booking someone's time off approves it at once; events follow.
    let booked = s
        .ok(
            &s.owner,
            "POST",
            &ask,
            Some(json!({ "startsOn": "2099-08-01", "endsOn": "2099-08-01", "kind": "training" })),
        )
        .await;
    assert_eq!(booked["status"], "approved");
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events_outbox WHERE payload->>'time_off_id' = ANY($1) ORDER BY occurred_at, id",
    )
    .bind(vec![id.to_owned(), booked["id"].as_str().unwrap().to_owned()])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        events,
        [
            "time_off.requested",
            "time_off.approved",
            "time_off.cancelled",
            "time_off.approved"
        ]
    );
}

#[tokio::test]
async fn fit_puts_available_people_first() {
    let Some(s) = setup().await else { return };
    let skill = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/skills",
            Some(json!({ "name": unique("Backups") })),
        )
        .await;
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Backups") })),
        )
        .await;
    let qid = queue["id"].as_str().unwrap();
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/workforce/requirements/queue/{qid}"),
        Some(json!([{ "skillId": skill["id"], "minLevel": 2 }])),
    )
    .await;
    // All three know backups; the expert is away, the proficient one is off shift.
    let (away, off, here) = (
        user(&s.pools, Some(("technician", s.provider))).await.0,
        user(&s.pools, Some(("technician", s.provider))).await.0,
        user(&s.pools, Some(("technician", s.provider))).await.0,
    );
    for (who, level) in [(away, 4), (off, 3), (here, 2)] {
        s.ok(
            &s.owner,
            "PUT",
            &format!(
                "/api/v1/workforce/people/{who}/skills/{}",
                skill["id"].as_str().unwrap()
            ),
            Some(json!({ "level": level })),
        )
        .await;
    }
    // Monday 2026-11-30, 10:00 UTC.
    let at = "2026-11-30T10:00:00Z";
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/workforce/people/{away}/time-off"),
        Some(json!({ "startsOn": "2026-11-30", "endsOn": "2026-12-01", "kind": "sick" })),
    )
    .await;
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/workforce/people/{off}/schedule"),
        Some(json!({ "timezone": "UTC", "hours": { "mon": [["13:00", "21:00"]] } })),
    )
    .await;
    let fits = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/workforce/fit?queueId={qid}&at={at}"),
            None,
        )
        .await;
    let ours: Vec<&Value> = fits
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| [away, off, here].iter().any(|u| f["userId"] == json!(u)))
        .collect();
    assert_eq!(ours[0]["userId"], json!(here));
    assert_eq!(ours[0]["available"], true);
    let reasons: Vec<(Value, Value)> = ours[1..]
        .iter()
        .map(|f| (f["userId"].clone(), f["unavailable"].clone()))
        .collect();
    assert!(
        reasons.contains(&(json!(away), json!("away"))),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&(json!(off), json!("off_hours"))),
        "{reasons:?}"
    );
    let (status, _) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/workforce/fit?queueId={qid}&at=noon"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn skill_gaps_show_thin_coverage_unqualified_work_and_expiring_certifications() {
    // Insights look at everything: a database of its own.
    let Some(db) = isolated("skill_gaps").await else {
        return;
    };
    let s = setup_with(db.pools.clone()).await;
    let cert = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workforce/certifications",
            Some(json!({ "vendor": "Cisco", "name": "CCNA" })),
        )
        .await;
    let (ada, ada_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let (bob, _) = user(&s.pools, Some(("technician", s.provider))).await;
    for (id, name) in [(ada, "Ada"), (bob, "Bob")] {
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&s.pools.owner)
            .await
            .unwrap();
    }
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": "Network", "userIds": [ada, bob] })),
        )
        .await;
    let qid = queue["id"].as_str().unwrap();
    s.ok(
        &s.owner,
        "PUT",
        &format!("/api/v1/workforce/requirements/queue/{qid}"),
        Some(json!([{ "certificationId": cert["id"] }])),
    )
    .await;
    let soon = (time::OffsetDateTime::now_utc() + time::Duration::days(20))
        .date()
        .to_string();
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/workforce/people/{ada}/certifications"),
        Some(json!({ "certificationId": cert["id"], "earnedOn": "2024-01-01", "expiresOn": soon })),
    )
    .await;
    // Bob resolved one of the queue's tickets without the certification.
    let ticket = s
        .ok(&s.owner, "POST", "/api/v1/tickets", Some(json!({ "orgId": s.provider, "title": "Router", "queueId": qid, "assigneeUserId": bob })))
        .await;
    sqlx::query("UPDATE sd_tickets SET resolved_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(ticket["id"].as_str().unwrap()).unwrap())
        .execute(&s.pools.owner)
        .await
        .unwrap();

    let tech = login(&s.app, &ada_email).await;
    assert_eq!(
        s.call(&tech, "GET", "/api/v1/workforce/insights", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let found = s
        .ok(&s.owner, "GET", "/api/v1/workforce/insights", None)
        .await;
    assert_eq!(found["coverage"][0]["qualified"], json!(["Ada"]), "{found}");
    assert_eq!(found["coverage"][0]["pool"], 2);
    assert_eq!(
        (
            &found["unqualified"][0]["tickets"],
            &found["unqualified"][0]["people"]
        ),
        (&json!(1), &json!(["Bob"]))
    );
    assert_eq!(found["expiring"][0]["uncovers"], json!(["Network"]));
    assert_eq!(
        found["summary"],
        json!([
            format!("Ada's Cisco CCNA expires {soon}: nobody else is qualified for Network"),
            "Only Ada meets everything Network requires",
            "1 recent ticket was worked without Cisco CCNA (by Bob)",
        ])
    );
}
