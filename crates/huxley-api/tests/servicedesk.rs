//! The `servicedesk` module's API against a real PostgreSQL: tickets and
//! their lifecycle, statuses and their rules, the priority matrix, notes and
//! activity, queues, permissions, scoping and the module gate.

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

const PASSWORD: &str = "desk api passphrase";

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
    let provider = org(&pools, root, huxley_orgs::OrgKind::Provider, "Desk MSP").await;
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

/// Turns a module off for a provider, first turning off for it the
/// enabled modules that need it (other tests turn modules on instance-wide).
async fn narrow_off(pools: &TestPools, provider: Uuid, module: &'static str) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut pending = vec![module];
    while let Some(&key) = pending.last() {
        match huxley_modules::set_for_org(&mut tx, &ctx, provider, key, false).await {
            Ok(_) => {
                pending.pop();
            }
            Err(huxley_modules::SetError::Module(huxley_modules::ModuleError::HasDependents(
                needing,
            ))) => pending.extend(needing),
            Err(e) => panic!("turning {key} off: {e}"),
        }
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn priority_follows_the_matrix_until_chosen() {
    let Some(s) = setup().await else { return };
    let contact = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.acme),
            Some(
                json!({ "firstName": "Ada", "lastName": "Lovelace", "email": "ada@acme.example" }),
            ),
        )
        .await;
    // Created in the type's first status, numbered with the prefix, priority from the matrix.
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "  Printer on fire  ", "impact": 1,
                                  "urgency": 2, "requesterId": contact["id"] }),
        )
        .await;
    assert_eq!(t["title"], "Printer on fire");
    assert_eq!(
        (t["typeKey"].as_str(), t["statusKey"].as_str()),
        (Some("incident"), Some("new"))
    );
    assert_eq!(
        (t["priority"].as_str(), t["priorityOverridden"].as_bool()),
        (Some("p2"), Some(false))
    );
    assert_eq!(t["requesterName"], "Ada Lovelace");
    assert!(
        t["reference"].as_str().unwrap().starts_with("T-"),
        "{}",
        t["reference"]
    );
    let url = url(&t);

    // Changing impact re-derives it; choosing one sticks; `auto` follows the matrix again.
    let patch = |t: &Value, change: Value| {
        let mut body = change;
        body["version"] = t["version"].clone();
        body
    };
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "impact": 3 }))),
        )
        .await;
    assert_eq!(t["priority"], "p4");
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "priority": "p1" }))),
        )
        .await;
    assert_eq!(
        (t["priority"].as_str(), t["priorityOverridden"].as_bool()),
        (Some("p1"), Some(true))
    );
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "urgency": 1 }))),
        )
        .await;
    assert_eq!(t["priority"], "p1");
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "priority": "auto" }))),
        )
        .await;
    assert_eq!(
        (t["priority"].as_str(), t["priorityOverridden"].as_bool()),
        (Some("p3"), Some(false))
    );
}

#[tokio::test]
async fn edits_need_the_version_and_null_clears() {
    let Some(s) = setup().await else { return };
    let contact = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.acme),
            Some(json!({ "firstName": "Edith" })),
        )
        .await;
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Edits", "requesterId": contact["id"] }),
        )
        .await;
    let url = url(&t);
    let patch = |t: &Value, change: Value| {
        let mut body = change;
        body["version"] = t["version"].clone();
        body
    };
    // A stale version conflicts; missing keeps the requester, null clears it.
    let (status, _) = s
        .call(
            &s.owner,
            "PATCH",
            &url,
            Some(json!({ "version": 99, "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "title": "Toner fire" }))),
        )
        .await;
    assert_eq!(t["requesterId"], contact["id"]);
    let t = s
        .ok(
            &s.owner,
            "PATCH",
            &url,
            Some(patch(&t, json!({ "requesterId": null }))),
        )
        .await;
    assert_eq!(t["requesterId"], Value::Null);
}

#[tokio::test]
async fn status_changes_follow_the_rules() {
    let Some(s) = setup().await else { return };
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Status rules" }),
        )
        .await;
    let status_url = format!("{}/status", url(&t));
    let to = |t: &Value, status: &str| json!({ "status": status, "version": t["version"] });

    // Resolved needs a resolution; given with the change, it's added as a note.
    let (status, problem) = s
        .call(&s.owner, "POST", &status_url, Some(to(&t, "resolved")))
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/resolution".into())
    );
    let (status, problem) = s
        .call(&s.owner, "POST", &status_url, Some(to(&t, "nope")))
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/status".into())
    );
    let mut resolve = to(&t, "resolved");
    resolve["resolution"] = json!("Replaced the fuser.");
    let t = s.ok(&s.owner, "POST", &status_url, Some(resolve)).await;
    assert_eq!(t["statusCategory"], "resolved");
    assert!(t["resolvedAt"].is_string() && t["closedAt"].is_null());
    let notes = s
        .ok(&s.owner, "GET", &format!("{}/notes", url(&t)), None)
        .await;
    assert_eq!(notes[0]["kind"], "resolution");

    // Reopening clears resolved; closing sets both; a stale version conflicts.
    let t = s
        .ok(&s.owner, "POST", &status_url, Some(to(&t, "in_progress")))
        .await;
    assert!(t["resolvedAt"].is_null());
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &status_url,
            Some(json!({ "status": "closed", "version": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let t = s
        .ok(&s.owner, "POST", &status_url, Some(to(&t, "closed")))
        .await;
    assert!(t["resolvedAt"].is_string() && t["closedAt"].is_string());
}

#[tokio::test]
async fn notes_activity_and_events() {
    let Some(s) = setup().await else { return };
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Notes" }))
        .await;
    let url = url(&t);
    let notes = format!("{url}/notes");

    // The first public note from staff is the first response.
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            &notes,
            Some(json!({ "kind": "secret", "body": "x" })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/kind".into())
    );
    s.ok(
        &s.owner,
        "POST",
        &notes,
        Some(json!({ "kind": "internal", "body": "Hmm" })),
    )
    .await;
    assert!(s.ok(&s.owner, "GET", &url, None).await["firstResponseAt"].is_null());
    let note = s
        .ok(
            &s.owner,
            "POST",
            &notes,
            Some(json!({ "kind": "public", "body": "On it" })),
        )
        .await;
    assert_eq!(note["authorName"], "Desk User");
    assert!(s.ok(&s.owner, "GET", &url, None).await["firstResponseAt"].is_string());
    let listed = s.ok(&s.owner, "GET", &notes, None).await;
    let kinds: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["internal", "public"]);

    // The timeline, the audit log and the events.
    let activity = s
        .ok(&s.owner, "GET", &format!("{url}/activity"), None)
        .await;
    let kinds: Vec<&str> = activity
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["created", "note", "note"]);
    let id = Uuid::parse_str(t["id"].as_str().unwrap()).unwrap();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events_outbox WHERE subject_id = $1 ORDER BY occurred_at, id",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        events,
        ["ticket.created", "ticket.note_added", "ticket.note_added"]
    );
    let audited: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_events WHERE target_id = $1 ORDER BY seq")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(audited, events);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn validation_and_references() {
    let Some(s) = setup().await else { return };
    for (body, pointer) in [
        (json!({ "orgId": s.acme, "title": " " }), "/title"),
        (
            json!({ "orgId": s.acme, "title": "x", "type": "nope" }),
            "/type",
        ),
        (
            json!({ "orgId": s.acme, "title": "x", "impact": 4 }),
            "/impact",
        ),
        (
            json!({ "orgId": s.acme, "title": "x", "priority": "p9" }),
            "/priority",
        ),
        (
            json!({ "orgId": s.acme, "title": "x", "source": "fax" }),
            "/source",
        ),
        (
            json!({ "orgId": s.acme, "title": "x", "queueId": Uuid::now_v7() }),
            "/queueId",
        ),
        (
            json!({ "orgId": s.acme, "title": "x", "assigneeUserId": Uuid::now_v7() }),
            "/assigneeUserId",
        ),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", "/api/v1/tickets", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} {problem}");
        assert_eq!(field(&problem), pointer, "{body}");
    }
    // A requester must be a contact at the ticket's organization.
    let globex_contact = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.globex),
            Some(json!({ "firstName": "Hank" })),
        )
        .await;
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.acme, "title": "x", "requesterId": globex_contact["id"] })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/requesterId".into())
    );
    // A service request, by type key; a chosen priority sticks from the start.
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "New laptop", "type": "service_request",
                                  "priority": "p4", "source": "phone" }),
        )
        .await;
    assert_eq!(
        (t["typeName"].as_str(), t["priority"].as_str()),
        (Some("Service request"), Some("p4"))
    );
    assert_eq!(
        (t["priorityOverridden"].as_bool(), t["source"].as_str()),
        (Some(true), Some("phone"))
    );

    // Configuration is readable.
    let types = s
        .ok(&s.owner, "GET", "/api/v1/servicedesk/types", None)
        .await;
    let incident = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "incident")
        .unwrap();
    assert_eq!(incident["statuses"].as_array().unwrap().len(), 6);
    let matrix = s
        .ok(&s.owner, "GET", "/api/v1/servicedesk/priority-matrix", None)
        .await;
    assert_eq!(matrix.as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn queues() {
    let Some(s) = setup().await else { return };
    let (member, _) = user(&s.pools, Some(("technician", s.provider))).await;
    let name = unique("Helpdesk");
    let q = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": name, "userIds": [member] })),
        )
        .await;
    assert_eq!(q["userIds"], json!([member]));
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": name })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/name".into())
    );
    let qid = q["id"].as_str().unwrap();
    let qurl = format!("/api/v1/queues/{qid}");

    // Renaming needs the version; members are replaced only when given.
    let renamed = format!("{name} 2");
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &qurl,
            Some(json!({ "name": renamed, "version": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let q = s
        .ok(
            &s.owner,
            "PUT",
            &qurl,
            Some(json!({ "name": renamed, "version": q["version"] })),
        )
        .await;
    assert_eq!(
        (q["name"].as_str(), q["userIds"].clone()),
        (Some(renamed.as_str()), json!([member]))
    );

    // Its open tickets are counted.
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Queued", "queueId": qid }),
        )
        .await;
    assert_eq!(t["queueName"], renamed.as_str());
    assert_eq!(s.ok(&s.owner, "GET", &qurl, None).await["openTickets"], 1);

    // Archiving hides it from the list and from new tickets.
    assert_eq!(
        s.call(&s.owner, "DELETE", &qurl, None).await.0,
        StatusCode::NO_CONTENT
    );
    let queues = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/queues?orgId={}", s.provider),
            None,
        )
        .await;
    assert!(!queues.as_array().unwrap().iter().any(|x| x["id"] == qid));
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.acme, "title": "x", "queueId": qid })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn queue_dispatch_setting() {
    let Some(s) = setup().await else { return };
    let name = unique("Dispatch");
    let q = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": name })),
        )
        .await;
    let qurl = format!("/api/v1/queues/{}", q["id"].as_str().unwrap());
    // Dispatch: manual unless chosen, checked, and kept when not given.
    assert_eq!(q["dispatch"], "manual");
    let (status, problem) = s
        .call(
            &s.owner,
            "PUT",
            &qurl,
            Some(json!({ "name": name, "version": q["version"], "dispatch": "lottery" })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/dispatch".into())
    );
    let q = s
        .ok(
            &s.owner,
            "PUT",
            &qurl,
            Some(json!({ "name": name, "version": q["version"], "dispatch": "load" })),
        )
        .await;
    let q = s
        .ok(
            &s.owner,
            "PUT",
            &qurl,
            Some(json!({ "name": name, "version": q["version"] })),
        )
        .await;
    assert_eq!(q["dispatch"], "load");
}

#[tokio::test]
async fn listing_filters() {
    let Some(s) = setup().await else { return };
    let q = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Listing") })),
        )
        .await;
    let title = unique("Listed ticket");
    let words = title.replace(' ', "+");
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": title, "queueId": q["id"] }),
        )
        .await;
    let total = |query: String| {
        let s = &s;
        async move {
            s.ok(&s.owner, "GET", &format!("/api/v1/tickets?{query}"), None)
                .await["total"]
                .clone()
        }
    };
    assert_eq!(
        total(format!("queueId={}", q["id"].as_str().unwrap())).await,
        1
    );
    assert_eq!(total(format!("search={words}")).await, 1);
    assert_eq!(
        total(format!("search={}", t["reference"].as_str().unwrap())).await,
        1
    );
    assert_eq!(
        total(format!("orgId={}&assignee=none&search={words}", s.acme)).await,
        1
    );
    assert_eq!(total(format!("orgId={}&search={words}", s.globex)).await, 0);
    assert_eq!(total(format!("search={words}&state=closed")).await, 0);
    assert_eq!(total(format!("search={words}&state=all")).await, 1);
    let (status, _) = s
        .call(&s.owner, "GET", "/api/v1/tickets?state=sideways", None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Assigning to me shows under assignee=me, and is an "assigned" change.
    assert_eq!(total(format!("assignee=me&search={words}")).await, 0);
    let me = s.ok(&s.owner, "GET", "/api/v1/auth/me", None).await["id"].clone();
    s.ok(
        &s.owner,
        "PATCH",
        &url(&t),
        Some(json!({ "version": t["version"], "assigneeUserId": me })),
    )
    .await;
    assert_eq!(total(format!("assignee=me&search={words}")).await, 1);
    assert_eq!(total(format!("assignee=none&search={words}")).await, 0);
    let activity = s
        .ok(&s.owner, "GET", &format!("{}/activity", url(&t)), None)
        .await;
    assert_eq!(
        activity.as_array().unwrap().last().unwrap()["kind"],
        "assigned"
    );
}

#[tokio::test]
async fn permissions_and_scoping() {
    let Some(s) = setup().await else { return };
    let acme = url(&s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": unique("Acme") }),
        )
        .await);
    let globex = url(&s
        .ticket(
            &s.owner,
            json!({ "orgId": s.globex, "title": unique("Globex") }),
        )
        .await);

    // A technician at Acme works Acme's tickets; Globex's don't exist for them.
    let tech = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.acme))).await.1,
    )
    .await;
    s.ok(
        &tech,
        "POST",
        &format!("{acme}/notes"),
        Some(json!({ "kind": "internal", "body": "Looking" })),
    )
    .await;
    assert_eq!(
        s.call(&tech, "GET", &globex, None).await.0,
        StatusCode::NOT_FOUND
    );
    let page = s
        .ok(&tech, "GET", "/api/v1/tickets?state=all&limit=200", None)
        .await;
    let items = page["items"].as_array().unwrap();
    assert!(items.iter().all(|t| t["orgId"] == json!(s.acme)));
    assert!(items.iter().any(|t| url(t) == acme));
    // ...but can't set up queues.
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.acme, "name": unique("Q") })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Read-only reads but doesn't write.
    let reader = login(&s.app, &user(&s.pools, Some(("read_only", s.acme))).await.1).await;
    s.ok(&reader, "GET", &acme, None).await;
    let (status, _) = s
        .call(
            &reader,
            "POST",
            &format!("{acme}/notes"),
            Some(json!({ "kind": "public", "body": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .call(
            &reader,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.acme, "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A missing ticket is a 404.
    let (status, _) = s
        .call(
            &s.owner,
            "GET",
            &format!("/api/v1/tickets/{}", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn module_gate() {
    let Some(s) = setup().await else { return };
    let t = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": unique("Gated") }),
        )
        .await;

    // Off for this provider and its clients: the routes don't exist there, and the list leaves them out.
    narrow_off(&s.pools, s.provider, "servicedesk").await;
    let (status, problem) = s.call(&s.owner, "GET", &url(&t), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        problem["type"],
        "https://usehuxley.io/problems/module-disabled"
    );
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/tickets",
            Some(json!({ "orgId": s.acme, "title": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let page = s
        .ok(&s.owner, "GET", "/api/v1/tickets?state=all&limit=200", None)
        .await;
    assert!(
        !page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == t["id"])
    );
}

/// Runs SQL in system scope with uuid binds.
async fn exec(pools: &TestPools, sql: &str, binds: &[Uuid]) {
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()));
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}

/// The requester adds a public note to a ticket at Acme.
async fn requester_wrote(s: &Setup, ticket: Uuid) {
    let contact = Uuid::now_v7();
    exec(
        &s.pools,
        "INSERT INTO crm_contacts (id, org_id, first_name) VALUES ($1, $2, 'Ada')",
        &[contact, s.acme],
    )
    .await;
    exec(
        &s.pools,
        "INSERT INTO sd_ticket_notes (id, ticket_id, org_id, kind, body, author_contact_id) \
         VALUES (gen_random_uuid(), $1, $2, 'public', 'Still broken', $3)",
        &[ticket, s.acme, contact],
    )
    .await;
}

/// A ticket at Acme misses its resolution target.
async fn breach(s: &Setup, ticket: Uuid) {
    exec(
        &s.pools,
        "INSERT INTO sd_ticket_sla (ticket_id, org_id, policy_name, calendar, targets, resolution_breached_at) \
         VALUES ($1, $2, 'Test', '{\"kind\": \"always\"}', '{}', now()) \
         ON CONFLICT (ticket_id) DO UPDATE SET resolution_breached_at = now()",
        &[ticket, s.acme],
    )
    .await;
}

#[tokio::test]
async fn my_work_puts_what_needs_attention_first() {
    let Some(s) = setup().await else { return };
    let (tech, email) = user(&s.pools, Some(("technician", s.provider))).await;
    let cookie = login(&s.app, &email).await;
    let other = user(&s.pools, Some(("technician", s.provider))).await.0;
    let team = Uuid::now_v7();
    exec(
        &s.pools,
        "INSERT INTO teams (id, name) VALUES ($1, 'Desk ' || $1::text)",
        &[team],
    )
    .await;
    exec(
        &s.pools,
        "INSERT INTO team_members (team_id, user_id) VALUES ($1, $2)",
        &[team, tech],
    )
    .await;
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Mine"), "teamIds": [team] })),
        )
        .await;
    let qid = queue["id"].as_str().unwrap();
    let mine = |title: &str| json!({ "orgId": s.acme, "title": title, "assigneeUserId": tech });
    let working = s.ticket(&s.owner, mine("Working on it")).await;
    let fresh = s.ticket(&s.owner, mine("Just in")).await;
    let replied = s.ticket(&s.owner, mine("They wrote back")).await;
    let breached = s.ticket(&s.owner, mine("Overdue")).await;
    let id = |t: &Value| t["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    for t in [&working, &replied] {
        s.ok(&cookie, "POST", &format!("{}/status", url(t)),
             Some(json!({ "status": "in_progress", "version": s.ok(&cookie, "GET", &url(t), None).await["version"] })))
            .await;
    }
    // The requester's public note is the latest on one; another is overdue.
    requester_wrote(&s, id(&replied)).await;
    breach(&s, id(&breached)).await;
    // Not mine: someone else's, the team's (nobody in it), the queue's.
    s.ticket(
        &s.owner,
        json!({ "orgId": s.acme, "title": "Theirs", "assigneeUserId": other }),
    )
    .await;
    let teams = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Team's", "assigneeTeamId": team }),
        )
        .await;
    let waiting = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.globex, "title": "Waiting", "queueId": qid }),
        )
        .await;

    let work = s.ok(&cookie, "GET", "/api/v1/my-work", None).await;
    let titles: Vec<&str> = work["mine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Overdue", "They wrote back", "Just in", "Working on it"]
    );
    assert_eq!(
        work["attention"],
        json!({
            breached["id"].as_str().unwrap(): "breached",
            replied["id"].as_str().unwrap(): "replied",
            fresh["id"].as_str().unwrap(): "new",
        })
    );
    assert_eq!(
        work["team"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["id"])
            .collect::<Vec<_>>(),
        [&teams["id"]]
    );
    let queues = work["queues"].as_array().unwrap();
    assert_eq!(queues.len(), 1);
    assert_eq!(
        (
            queues[0]["unassigned"].as_i64(),
            &queues[0]["tickets"][0]["id"]
        ),
        (Some(1), &waiting["id"])
    );
}
