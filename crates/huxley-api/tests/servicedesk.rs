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
    for module in ["crm", "servicedesk", "change"] {
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
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE target_id = $1 ORDER BY occurred_at, id",
    )
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

/// Drops tickets' SLA clocks: a policy another test left in the shared
/// database would make them "due soon" at some times of day.
async fn no_sla_clocks(s: &Setup, tickets: &[Uuid]) {
    for t in tickets {
        exec(
            &s.pools,
            "DELETE FROM sd_ticket_sla WHERE ticket_id = $1",
            &[*t],
        )
        .await;
    }
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

    no_sla_clocks(&s, &[id(&working), id(&fresh)]).await;
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

#[tokio::test]
async fn service_desk_notification_kinds_are_offered_while_it_is_on() {
    let Some(s) = setup().await else { return };
    let prefs = s
        .ok(&s.owner, "GET", "/api/v1/notifications/preferences", None)
        .await;
    let kinds: Vec<&str> = prefs
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "ticket.assigned",
        "ticket.replied",
        "ticket.sla",
        "workflow.notification",
    ] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
}

#[tokio::test]
async fn canned_responses_fill_in_the_ticket_and_technician() {
    let Some(s) = setup().await else { return };
    let (_, email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &email).await;
    let name = unique("Password reset");
    let created = s
        .ok(&s.owner, "POST", "/api/v1/canned-responses", Some(json!({
            "name": name, "category": "Accounts",
            "body": "Hi {{requester.first_name}},\n\nYour password for {{ client.name }} is reset ({{ticket.reference}}). {{nope}}\n\n{{me.first_name}}",
        })))
        .await;
    assert_eq!(created["kind"], "public");
    // Technicians use them; only configurers manage them.
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/canned-responses",
            Some(json!({ "name": "x", "body": "y" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let contact = s
        .ok(
            &s.owner,
            "POST",
            &format!("/api/v1/orgs/{}/contacts", s.acme),
            Some(json!({ "firstName": "Ada", "lastName": "Lovelace" })),
        )
        .await;
    let ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Locked out", "requesterId": contact["id"] }),
        )
        .await;
    let filled = s
        .ok(&tech, "GET", &format!("{}/responses", url(&ticket)), None)
        .await;
    let mine = filled
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name.as_str())
        .unwrap();
    let reference = ticket["reference"].as_str().unwrap();
    let expected = format!(
        "Hi Ada,\n\nYour password for {} is reset ({reference}). {{{{nope}}}}\n\n",
        ticket["orgName"].as_str().unwrap()
    );
    assert!(
        mine["body"].as_str().unwrap().starts_with(&expected),
        "{}",
        mine["body"]
    );

    // Changing needs the version; archiving removes it from tickets.
    let rurl = format!(
        "/api/v1/canned-responses/{}",
        created["id"].as_str().unwrap()
    );
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &rurl,
            Some(json!({ "name": name, "body": "New", "version": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let changed = s
        .ok(&s.owner, "PUT", &rurl, Some(json!({ "name": name, "body": "New", "kind": "internal", "version": created["version"] })))
        .await;
    assert_eq!(
        (changed["body"].as_str(), changed["kind"].as_str()),
        (Some("New"), Some("internal"))
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &rurl, None).await.0,
        StatusCode::NO_CONTENT
    );
    let after = s
        .ok(&tech, "GET", &format!("{}/responses", url(&ticket)), None)
        .await;
    assert!(
        !after
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == name.as_str())
    );
}

#[tokio::test]
async fn canned_responses_are_checked() {
    let Some(s) = setup().await else { return };
    let name = unique("Taken");
    s.ok(
        &s.owner,
        "POST",
        "/api/v1/canned-responses",
        Some(json!({ "name": name, "body": "x" })),
    )
    .await;
    for (body, pointer) in [
        (json!({ "name": name, "body": "again" }), "/name"),
        (json!({ "name": "Empty", "body": "  " }), "/body"),
        (
            json!({ "name": "Odd", "body": "x", "kind": "shout" }),
            "/kind",
        ),
    ] {
        let (status, problem) = s
            .call(
                &s.owner,
                "POST",
                "/api/v1/canned-responses",
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
}

#[tokio::test]
async fn tickets_link_both_ways_without_loops() {
    let Some(s) = setup().await else { return };
    let t = |title: &str| json!({ "orgId": s.acme, "title": title });
    let (outage, a, b) = (
        s.ticket(&s.owner, t("Email outage")).await,
        s.ticket(&s.owner, t("Can't send")).await,
        s.ticket(&s.owner, t("Can't receive")).await,
    );
    let links = |x: &Value| format!("{}/links", url(x));
    // a and b are children of the outage (one by id, one by reference).
    s.ok(
        &s.owner,
        "POST",
        &links(&a),
        Some(json!({ "otherId": outage["id"], "kind": "parent" })),
    )
    .await;
    let listed = s
        .ok(
            &s.owner,
            "POST",
            &links(&outage),
            Some(json!({ "reference": b["reference"], "kind": "child" })),
        )
        .await;
    let kinds: Vec<(&str, &str)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["kind"].as_str().unwrap(), l["title"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, [("child", "Can't send"), ("child", "Can't receive")]);
    let up = s.ok(&s.owner, "GET", &links(&b), None).await;
    assert_eq!(
        (up[0]["kind"].as_str(), up[0]["id"].clone()),
        (Some("parent"), outage["id"].clone())
    );
    // No loops, one parent, not itself, real references.
    for (from, body, pointer) in [
        (
            &outage,
            json!({ "otherId": a["id"], "kind": "parent" }),
            "/kind",
        ),
        (&a, json!({ "otherId": b["id"], "kind": "parent" }), "/kind"),
        (&a, json!({ "otherId": a["id"] }), "/reference"),
        (&a, json!({ "reference": "T-999999999" }), "/reference"),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", &links(from), Some(body.clone()))
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
    // Related works across organizations; unlinking removes both sides.
    let elsewhere = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.globex, "title": "Same outage" }),
        )
        .await;
    s.ok(
        &s.owner,
        "POST",
        &links(&a),
        Some(json!({ "otherId": elsewhere["id"] })),
    )
    .await;
    assert_eq!(
        s.ok(&s.owner, "GET", &links(&elsewhere), None).await[0]["kind"],
        "related"
    );
    let gone = format!("{}/{}", links(&a), elsewhere["id"].as_str().unwrap());
    assert_eq!(
        s.call(&s.owner, "DELETE", &gone, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        s.ok(&s.owner, "GET", &links(&elsewhere), None).await,
        json!([])
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &gone, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn merging_moves_the_conversation_and_closes_the_duplicate() {
    let Some(s) = setup().await else { return };
    let (keep, dup) = (
        s.ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Printer offline" }),
        )
        .await,
        s.ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Printer offline again" }),
        )
        .await,
    );
    s.ok(
        &s.owner,
        "POST",
        &format!("{}/notes", url(&dup)),
        Some(json!({ "kind": "internal", "body": "Toner low" })),
    )
    .await;
    let merge = |t: &Value| format!("{}/merge", url(t));
    let merged = s
        .ok(
            &s.owner,
            "POST",
            &merge(&dup),
            Some(json!({ "into": keep["reference"] })),
        )
        .await;
    assert_eq!(merged["id"], keep["id"]);
    let notes = s
        .ok(&s.owner, "GET", &format!("{}/notes", url(&keep)), None)
        .await;
    assert!(
        notes
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["body"] == "Toner low"),
        "{notes}"
    );
    let dup = s.ok(&s.owner, "GET", &url(&dup), None).await;
    assert_eq!(
        (
            dup["statusCategory"].as_str(),
            dup["mergedIntoReference"].clone()
        ),
        (Some("closed"), keep["reference"].clone())
    );
    // Not again, not into itself, not across organizations.
    let other = s
        .ticket(&s.owner, json!({ "orgId": s.globex, "title": "Theirs" }))
        .await;
    let fresh = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Fresh" }))
        .await;
    for (from, body) in [
        (&dup, json!({ "intoId": keep["id"] })),
        (&fresh, json!({ "intoId": fresh["id"] })),
        (&fresh, json!({ "intoId": other["id"] })),
        (&fresh, json!({ "intoId": dup["id"] })),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", &merge(from), Some(body.clone()))
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, "/into".into()),
            "{body}"
        );
    }
    // Technicians elsewhere can't merge into tickets they can't change.
    let (_, email) = user(&s.pools, Some(("technician", s.globex))).await;
    let theirs = login(&s.app, &email).await;
    let (status, _) = s
        .call(
            &theirs,
            "POST",
            &merge(&other),
            Some(json!({ "intoId": fresh["id"] })),
        )
        .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}

#[tokio::test]
async fn tickets_filter_by_priority() {
    let Some(s) = setup().await else { return };
    let tag = unique("Prio");
    for (impact, urgency) in [(1, 1), (2, 2), (3, 3)] {
        s.ticket(&s.owner, json!({ "orgId": s.globex, "title": format!("{tag} {impact}"), "impact": impact, "urgency": urgency }))
            .await;
    }
    let found = s
        .ok(
            &s.owner,
            "GET",
            &format!(
                "/api/v1/tickets?search={}&priority=p1,p3",
                tag.replace(' ', "%20")
            ),
            None,
        )
        .await;
    let priorities: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["priority"].as_str().unwrap())
        .collect();
    assert!(
        !priorities.is_empty() && priorities.iter().all(|p| *p == "p1" || *p == "p3"),
        "{priorities:?}"
    );
    let (status, problem) = s
        .call(&s.owner, "GET", "/api/v1/tickets?priority=p9", None)
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/priority".into())
    );
}

#[tokio::test]
async fn saved_views_are_yours_or_shared() {
    let Some(s) = setup().await else { return };
    let (_, email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &email).await;
    let name = unique("Urgent");
    let filters = json!({ "view": "open", "priority": "p1,p2", "queueId": "" });
    let mine = s
        .ok(
            &tech,
            "POST",
            "/api/v1/ticket-views",
            Some(json!({ "name": name, "filters": filters })),
        )
        .await;
    assert_eq!(
        (mine["shared"].as_bool(), mine["filters"].clone()),
        (Some(false), json!({ "view": "open", "priority": "p1,p2" }))
    );
    // Technicians can't share; configurers can. Others don't see a personal one.
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/ticket-views",
            Some(json!({ "name": "Team", "filters": {}, "shared": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let shared = s
        .ok(&s.owner, "POST", "/api/v1/ticket-views", Some(json!({ "name": unique("Team"), "filters": { "view": "unassigned" }, "shared": true })))
        .await;
    let theirs = s.ok(&tech, "GET", "/api/v1/ticket-views", None).await;
    let ids: Vec<&Value> = theirs
        .as_array()
        .unwrap()
        .iter()
        .map(|v| &v["id"])
        .collect();
    assert!(ids.contains(&&mine["id"]) && ids.contains(&&shared["id"]));
    let owners = s.ok(&s.owner, "GET", "/api/v1/ticket-views", None).await;
    assert!(
        !owners
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == mine["id"])
    );
    // The technician can't change the shared one, but can change theirs.
    let shared_url = format!("/api/v1/ticket-views/{}", shared["id"].as_str().unwrap());
    assert_eq!(
        s.call(&tech, "DELETE", &shared_url, None).await.0,
        StatusCode::FORBIDDEN
    );
    let mine_url = format!("/api/v1/ticket-views/{}", mine["id"].as_str().unwrap());
    let (status, _) = s
        .call(
            &tech,
            "PUT",
            &mine_url,
            Some(json!({ "name": name, "filters": {}, "version": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let renamed = s
        .ok(&tech, "PUT", &mine_url, Some(json!({ "name": format!("{name} 2"), "filters": { "view": "mine" }, "version": mine["version"] })))
        .await;
    assert_eq!(renamed["filters"], json!({ "view": "mine" }));
    assert_eq!(
        s.call(&s.owner, "DELETE", &mine_url, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.call(&tech, "DELETE", &mine_url, None).await.0,
        StatusCode::NO_CONTENT
    );
    for body in [
        json!({ "name": "Bad", "filters": { "view": "everything" } }),
        json!({ "name": "Bad", "filters": { "colour": "red" } }),
    ] {
        let (status, problem) = s
            .call(&tech, "POST", "/api/v1/ticket-views", Some(body.clone()))
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, "/filters".into()),
            "{body}"
        );
    }
}

#[tokio::test]
async fn search_finds_what_you_can_see() {
    let Some(s) = setup().await else { return };
    let word = format!("Zebrafish{}", &Uuid::now_v7().simple().to_string()[..8]);
    let acme_ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": format!("{word} printer jam") }),
        )
        .await;
    s.ticket(
        &s.owner,
        json!({ "orgId": s.globex, "title": format!("{word} VPN") }),
    )
    .await;
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/orgs/{}/contacts", s.acme),
        Some(json!({ "firstName": word, "lastName": "Okafor" })),
    )
    .await;
    let search = async |who: &str, q: &str| -> Vec<(String, String)> {
        let found = s
            .ok(
                who,
                "GET",
                &format!("/api/v1/search?q={}", q.replace(' ', "%20")),
                None,
            )
            .await;
        found
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["kind"].as_str().unwrap().to_owned(),
                    h["title"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let found = search(&s.owner, &word.to_lowercase()).await;
    assert!(
        found.contains(&("ticket".into(), format!("{word} printer jam"))),
        "{found:?}"
    );
    assert!(
        found.contains(&("ticket".into(), format!("{word} VPN"))),
        "{found:?}"
    );
    assert!(
        found.contains(&("contact".into(), format!("{word} Okafor"))),
        "{found:?}"
    );
    // By reference.
    let by_ref = search(&s.owner, acme_ticket["reference"].as_str().unwrap()).await;
    assert_eq!(
        by_ref.first(),
        Some(&("ticket".into(), format!("{word} printer jam")))
    );
    // A technician at Globex sees Globex's ticket only, and no contacts at Acme.
    let (_, email) = user(&s.pools, Some(("technician", s.globex))).await;
    let theirs = login(&s.app, &email).await;
    let found = search(&theirs, &word).await;
    assert!(
        found.contains(&("ticket".into(), format!("{word} VPN"))),
        "{found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(k, t)| (k == "ticket" && t.ends_with("printer jam")) || k == "contact"),
        "{found:?}"
    );
    let (status, problem) = s.call(&s.owner, "GET", "/api/v1/search?q=a", None).await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/q".into())
    );
}

#[tokio::test]
async fn bulk_edits_change_each_ticket_they_can() {
    let Some(s) = setup().await else { return };
    let (tech_id, email) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &email).await;
    let (a, b) = (
        s.ticket(&s.owner, json!({ "orgId": s.acme, "title": "Bulk A" }))
            .await,
        s.ticket(&s.owner, json!({ "orgId": s.acme, "title": "Bulk B" }))
            .await,
    );
    let theirs = s
        .ticket(&s.owner, json!({ "orgId": s.globex, "title": "Bulk C" }))
        .await;
    let results = s
        .ok(
            &tech,
            "POST",
            "/api/v1/tickets/bulk",
            Some(json!({
                "ids": [a["id"], b["id"], theirs["id"], a["id"]],
                "changes": { "assigneeUserId": tech_id, "priority": "p2" },
                "status": "in_progress",
            })),
        )
        .await;
    let outcomes: Vec<(Value, bool)> = results
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["id"].clone(), r["ok"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        outcomes,
        [
            (a["id"].clone(), true),
            (b["id"].clone(), true),
            (theirs["id"].clone(), false)
        ]
    );
    for t in [&a, &b] {
        let now = s.ok(&s.owner, "GET", &url(t), None).await;
        assert_eq!(
            (
                now["assigneeUserId"].clone(),
                now["priority"].as_str(),
                now["statusKey"].as_str()
            ),
            (json!(tech_id), Some("p2"), Some("in_progress"))
        );
    }
    let untouched = s.ok(&s.owner, "GET", &url(&theirs), None).await;
    assert!(untouched["assigneeUserId"].is_null());
    // A status they can't take is reported for each; clearing works.
    let results = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/tickets/bulk",
            Some(json!({ "ids": [a["id"]], "status": "resolved" })),
        )
        .await;
    assert_eq!(results[0]["ok"], false);
    assert!(
        results[0]["error"].as_str().unwrap().contains("resolution"),
        "{results}"
    );
    s.ok(
        &s.owner,
        "POST",
        "/api/v1/tickets/bulk",
        Some(json!({ "ids": [b["id"]], "changes": { "assigneeUserId": null } })),
    )
    .await;
    assert!(s.ok(&s.owner, "GET", &url(&b), None).await["assigneeUserId"].is_null());
    for (body, pointer) in [
        (json!({ "ids": [], "status": "in_progress" }), "/ids"),
        (json!({ "ids": [a["id"]] }), "/changes"),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", "/api/v1/tickets/bulk", Some(body.clone()))
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
}

#[tokio::test]
async fn ticket_templates_are_checked_and_managed_by_configurers() {
    let Some(s) = setup().await else { return };
    let (_, email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &email).await;
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Onboarding") })),
        )
        .await;
    let name = unique("New starter");
    let made = s
        .ok(&s.owner, "POST", "/api/v1/ticket-templates", Some(json!({
            "name": name, "typeKey": "service_request", "title": "New starter: ",
            "description": "Laptop, accounts, licences.", "impact": 3, "urgency": 2, "queueId": queue["id"],
        })))
        .await;
    assert_eq!(
        (made["typeKey"].as_str(), made["queueName"].clone()),
        (Some("service_request"), queue["name"].clone())
    );
    // Technicians read them; only configurers change them.
    let listed = s.ok(&tech, "GET", "/api/v1/ticket-templates", None).await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name.as_str())
    );
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/ticket-templates",
            Some(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (body, pointer) in [
        (json!({ "name": name }), "/name"),
        (json!({ "name": "Odd type", "typeKey": "nope" }), "/typeKey"),
        (json!({ "name": "Odd level", "impact": 7 }), "/impact"),
        (
            json!({ "name": "Odd queue", "queueId": Uuid::now_v7() }),
            "/queueId",
        ),
    ] {
        let (status, problem) = s
            .call(
                &s.owner,
                "POST",
                "/api/v1/ticket-templates",
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
    let turl = format!("/api/v1/ticket-templates/{}", made["id"].as_str().unwrap());
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &turl,
            Some(json!({ "name": name, "version": 99 })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let changed = s
        .ok(
            &s.owner,
            "PUT",
            &turl,
            Some(json!({ "name": name, "title": "Leaver: ", "version": made["version"] })),
        )
        .await;
    assert_eq!(
        (
            changed["title"].as_str(),
            changed["typeKey"].as_str(),
            changed["queueId"].clone()
        ),
        (Some("Leaver:"), Some("incident"), Value::Null)
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &turl, None).await.0,
        StatusCode::NO_CONTENT
    );
    let after = s.ok(&tech, "GET", "/api/v1/ticket-templates", None).await;
    assert!(
        !after
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name.as_str())
    );
}

/// A work type by name.
async fn work_type(s: &Setup, name: &str) -> Value {
    let types = s.ok(&s.owner, "GET", "/api/v1/work-types", None).await;
    types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn time_is_logged_on_tickets_by_whoever_did_it() {
    let Some(s) = setup().await else { return };
    let (tech_id, email) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &email).await;
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Time me" }))
        .await;
    let time = format!("{}/time", url(&t));
    let remote = work_type(&s, "Remote support").await;
    let travel = work_type(&s, "Travel").await;
    let logged = s
        .ok(&tech, "POST", &time, Some(json!({ "workTypeId": remote["id"], "minutes": 45, "notes": "Reset the VPN client." })))
        .await;
    assert_eq!(
        (
            logged["billable"].as_bool(),
            logged["userId"].clone(),
            logged["ticketReference"].clone()
        ),
        (Some(true), json!(tech_id), t["reference"].clone())
    );
    s.ok(
        &s.owner,
        "POST",
        &time,
        Some(json!({ "workTypeId": travel["id"], "minutes": 30, "workedOn": "2026-11-20" })),
    )
    .await;
    let listed = s.ok(&tech, "GET", &time, None).await;
    let minutes: i64 = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["minutes"].as_i64().unwrap())
        .sum();
    assert_eq!(minutes, 75);
    // Checks.
    let admin = work_type(&s, "Admin").await;
    for (body, pointer) in [
        (
            json!({ "workTypeId": admin["id"], "minutes": 10 }),
            "/workTypeId",
        ),
        (
            json!({ "workTypeId": Uuid::now_v7(), "minutes": 10 }),
            "/workTypeId",
        ),
        (
            json!({ "workTypeId": remote["id"], "minutes": 0 }),
            "/minutes",
        ),
        (
            json!({ "workTypeId": remote["id"], "minutes": 10, "workedOn": "2099-01-01" }),
            "/workedOn",
        ),
        (
            json!({ "workTypeId": remote["id"], "minutes": 10, "workedOn": "yesterday" }),
            "/workedOn",
        ),
    ] {
        let (status, problem) = s.call(&tech, "POST", &time, Some(body.clone())).await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
    // Yours to change; not someone else's; and not on tickets you can't reach.
    let eurl = format!("/api/v1/time-entries/{}", logged["id"].as_str().unwrap());
    let changed = s
        .ok(&tech, "PUT", &eurl, Some(json!({ "workTypeId": remote["id"], "minutes": 60, "billable": false, "version": logged["version"] })))
        .await;
    assert_eq!(
        (changed["minutes"].as_i64(), changed["billable"].as_bool()),
        (Some(60), Some(false))
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &eurl, None).await.0,
        StatusCode::NOT_FOUND
    );
    let elsewhere = s
        .ticket(&s.owner, json!({ "orgId": s.globex, "title": "Not yours" }))
        .await;
    let (status, _) = s
        .call(
            &tech,
            "POST",
            &format!("{}/time", url(&elsewhere)),
            Some(json!({ "workTypeId": remote["id"], "minutes": 5 })),
        )
        .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
    assert_eq!(
        s.call(&tech, "DELETE", &eurl, None).await.0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn work_types_are_managed_by_configurers() {
    let Some(s) = setup().await else { return };
    let (_, email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech = login(&s.app, &email).await;
    let name = unique("Weekend callout");
    let made = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/work-types",
            Some(json!({ "name": name, "billable": true })),
        )
        .await;
    assert_eq!(made["onTickets"], true);
    let (status, _) = s
        .call(
            &tech,
            "POST",
            "/api/v1/work-types",
            Some(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            "/api/v1/work-types",
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/name".into())
    );
    let wurl = format!("/api/v1/work-types/{}", made["id"].as_str().unwrap());
    let changed = s
        .ok(&s.owner, "PUT", &wurl, Some(json!({ "name": name, "billable": false, "onTickets": false, "version": made["version"] })))
        .await;
    assert_eq!(
        (
            changed["billable"].as_bool(),
            changed["onTickets"].as_bool()
        ),
        (Some(false), Some(false))
    );
    assert_eq!(
        s.call(&s.owner, "DELETE", &wurl, None).await.0,
        StatusCode::NO_CONTENT
    );
    let active = s.ok(&tech, "GET", "/api/v1/work-types", None).await;
    assert!(
        !active
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name.as_str())
    );
    let all = s
        .ok(&tech, "GET", "/api/v1/work-types?archived=true", None)
        .await;
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name.as_str() && t["archived"] == true)
    );
}

#[tokio::test]
async fn my_time_covers_tickets_and_the_rest() {
    let Some(s) = setup().await else { return };
    let (_, email) = user(&s.pools, Some(("technician", s.acme))).await;
    let tech = login(&s.app, &email).await;
    let (_, other_email) = user(&s.pools, Some(("technician", s.acme))).await;
    let other = login(&s.app, &other_email).await;
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Week" }))
        .await;
    let (remote, training) = (
        work_type(&s, "Remote support").await,
        work_type(&s, "Training").await,
    );
    let today = jiff::Zoned::now().date();
    let yesterday = today.yesterday().unwrap().to_string();
    s.ok(
        &tech,
        "POST",
        &format!("{}/time", url(&t)),
        Some(json!({ "workTypeId": remote["id"], "minutes": 90 })),
    )
    .await;
    let course = s
        .ok(&tech, "POST", "/api/v1/time-entries", Some(json!({ "workTypeId": training["id"], "minutes": 120, "workedOn": yesterday, "notes": "Intune course" })))
        .await;
    assert_eq!(
        (course["ticketId"].clone(), course["billable"].as_bool()),
        (Value::Null, Some(false))
    );
    s.ok(
        &other,
        "POST",
        "/api/v1/time-entries",
        Some(json!({ "workTypeId": training["id"], "minutes": 30 })),
    )
    .await;
    let week = format!("/api/v1/time-entries?from={yesterday}&to={today}");
    let mine = s.ok(&tech, "GET", &week, None).await;
    let rows: Vec<(Option<&str>, i64)> = mine
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["ticketReference"].as_str(),
                e["minutes"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [(None, 120), (t["reference"].as_str(), 90)]);
    for q in [
        format!("from={today}&to={yesterday}"),
        format!("from={yesterday}&to=2099-01-01"),
        "from=x&to=y".to_owned(),
    ] {
        let (status, _) = s
            .call(&tech, "GET", &format!("/api/v1/time-entries?{q}"), None)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
    }
    // Their own only.
    let eurl = format!("/api/v1/time-entries/{}", course["id"].as_str().unwrap());
    assert_eq!(
        s.call(&other, "DELETE", &eurl, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        s.call(&tech, "DELETE", &eurl, None).await.0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn presence_shows_who_else_is_on_a_ticket() {
    let Some(s) = setup().await else { return };
    let (ada_id, ada_email) = user(&s.pools, Some(("technician", s.acme))).await;
    let ada = login(&s.app, &ada_email).await;
    let (_, ben_email) = user(&s.pools, Some(("technician", s.acme))).await;
    let ben = login(&s.app, &ben_email).await;
    let t = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Who's here" }))
        .await;
    let here = format!("{}/presence", url(&t));
    assert_eq!(s.ok(&ada, "POST", &here, Some(json!({}))).await, json!([]));
    let seen = s
        .ok(&ben, "POST", &here, Some(json!({ "typing": true })))
        .await;
    assert_eq!(
        (seen[0]["userId"].clone(), seen[0]["typing"].as_bool()),
        (json!(ada_id), Some(false))
    );
    let seen = s.ok(&ada, "POST", &here, Some(json!({}))).await;
    assert_eq!(
        (seen.as_array().unwrap().len(), seen[0]["typing"].as_bool()),
        (1, Some(true))
    );
    // Ben leaves; then Ada is alone.
    s.ok(&ben, "POST", &here, Some(json!({ "leaving": true })))
        .await;
    assert_eq!(s.ok(&ada, "POST", &here, Some(json!({}))).await, json!([]));
    // Someone not heard from for a while has gone.
    s.ok(&ben, "POST", &here, Some(json!({}))).await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "UPDATE sd_ticket_presence SET seen_at = now() - interval '2 minutes' WHERE ticket_id = $1",
    )
    .bind(Uuid::parse_str(t["id"].as_str().unwrap()).unwrap())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(s.ok(&ada, "POST", &here, Some(json!({}))).await, json!([]));
    // Not on tickets you can't see.
    let theirs = s
        .ticket(&s.owner, json!({ "orgId": s.globex, "title": "Elsewhere" }))
        .await;
    let (status, _) = s
        .call(
            &ada,
            "POST",
            &format!("{}/presence", url(&theirs)),
            Some(json!({})),
        )
        .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}

#[tokio::test]
async fn appointments_book_people_once_and_show_on_the_board() {
    let Some(s) = setup().await else { return };
    let (tech_id, _) = user(&s.pools, Some(("technician", s.provider))).await;
    let (_, acme_email) = user(&s.pools, Some(("technician", s.acme))).await;
    let acme_tech = login(&s.app, &acme_email).await;
    let queue = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": unique("Field"), "userIds": [tech_id] })),
        )
        .await;
    let (a, g) = (
        s.ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Onsite: printer" }),
        )
        .await,
        s.ticket(
            &s.owner,
            json!({ "orgId": s.globex, "title": "Onsite: switch" }),
        )
        .await,
    );
    let book = |t: &Value| format!("{}/appointments", url(t));
    let slot = |from: &str, to: &str| json!({ "userId": tech_id, "startsAt": from, "endsAt": to, "notes": "Bring a ladder" });
    let first = s
        .ok(
            &s.owner,
            "POST",
            &book(&a),
            Some(slot("2099-03-02T09:00:00Z", "2099-03-02T11:00:00Z")),
        )
        .await;
    assert_eq!(first["ticketReference"], a["reference"]);
    // Not twice at once (even on another client's ticket); back to back is fine.
    let (status, problem) = s
        .call(
            &s.owner,
            "POST",
            &book(&g),
            Some(slot("2099-03-02T10:00:00Z", "2099-03-02T12:00:00Z")),
        )
        .await;
    assert_eq!(
        (status, field(&problem)),
        (StatusCode::UNPROCESSABLE_ENTITY, "/startsAt".into())
    );
    s.ok(
        &s.owner,
        "POST",
        &book(&g),
        Some(slot("2099-03-02T11:00:00Z", "2099-03-02T12:00:00Z")),
    )
    .await;
    // The board: the queue's member with both bookings; a technician at Acme
    // sees only Acme's.
    let board = format!(
        "/api/v1/dispatch/board?from=2099-03-02&days=7&queueId={}",
        queue["id"].as_str().unwrap()
    );
    let rows = s.ok(&s.owner, "GET", &board, None).await;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            rows[0]["userId"].clone(),
            rows[0]["appointments"].as_array().unwrap().len()
        ),
        (json!(tech_id), 2)
    );
    let theirs = s.ok(&acme_tech, "GET", &board, None).await;
    let refs: Vec<&Value> = theirs[0]["appointments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| &x["ticketReference"])
        .collect();
    assert_eq!(refs, [&a["reference"]]);
    let (status, _) = s
        .call(&s.owner, "GET", "/api/v1/dispatch/board?from=soon", None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Moved, then cancelled; the ticket's history has both.
    let aurl = format!("/api/v1/appointments/{}", first["id"].as_str().unwrap());
    let moved = s
        .ok(&s.owner, "PUT", &aurl, Some(json!({ "userId": tech_id, "startsAt": "2099-03-04T09:00:00Z", "endsAt": "2099-03-04T10:00:00Z", "version": first["version"] })))
        .await;
    assert_eq!(moved["startsAt"], "2099-03-04T09:00:00Z");
    assert_eq!(
        s.call(&s.owner, "DELETE", &aurl, None).await.0,
        StatusCode::NO_CONTENT
    );
    let activity = s
        .ok(&s.owner, "GET", &format!("{}/activity", url(&a)), None)
        .await;
    let kinds: Vec<&Value> = activity
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["kind"] == "appointment")
        .map(|x| &x["details"])
        .collect();
    assert_eq!(kinds.len(), 3, "{kinds:?}");
}

#[tokio::test]
async fn appointments_are_checked() {
    let Some(s) = setup().await else { return };
    let (tech_id, _) = user(&s.pools, Some(("technician", s.provider))).await;
    let a = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Checks" }))
        .await;
    let book = |t: &Value| format!("{}/appointments", url(t));
    let slot = |from: &str, to: &str| json!({ "userId": tech_id, "startsAt": from, "endsAt": to });
    for (body, pointer) in [
        (
            slot("2099-03-03T11:00:00Z", "2099-03-03T10:00:00Z"),
            "/endsAt",
        ),
        (
            slot("2099-03-03T09:00:00Z", "2099-03-05T09:00:00Z"),
            "/endsAt",
        ),
        (
            json!({ "userId": Uuid::now_v7(), "startsAt": "2099-03-03T09:00:00Z", "endsAt": "2099-03-03T10:00:00Z" }),
            "/userId",
        ),
    ] {
        let (status, problem) = s
            .call(&s.owner, "POST", &book(&a), Some(body.clone()))
            .await;
        assert_eq!(
            (status, field(&problem)),
            (StatusCode::UNPROCESSABLE_ENTITY, pointer.into()),
            "{body}"
        );
    }
}

/// Two queues at the provider: `(hardware, network)` as `(id, name)`.
async fn triage_queues(s: &Setup) -> ((String, String), (String, String)) {
    let mut out = Vec::new();
    for name in ["Hardware", "Network"] {
        let name = unique(name);
        let q = s
            .ok(
                &s.owner,
                "POST",
                "/api/v1/queues",
                Some(json!({ "orgId": s.provider, "name": name })),
            )
            .await;
        out.push((q["id"].as_str().unwrap().to_owned(), name));
    }
    let network = out.pop().unwrap();
    (out.pop().unwrap(), network)
}

#[tokio::test]
async fn ai_triage_applies_until_someone_works_the_ticket() {
    let Some(s) = setup().await else { return };
    let ((hw, hw_name), _) = triage_queues(&s).await;
    let ctx = RequestContext::system(Via::Workflow);
    let raised = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Laptop dead" }))
        .await;
    let worked = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Printer jam" }))
        .await;
    let raised_id = Uuid::parse_str(raised["id"].as_str().unwrap()).unwrap();
    let worked_id = Uuid::parse_str(worked["id"].as_str().unwrap()).unwrap();
    // As if a client raised it: no person has changed it.
    exec(
        &s.pools,
        "UPDATE sd_ticket_activity SET actor_id = NULL WHERE ticket_id = $1",
        &[raised_id],
    )
    .await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let applied = huxley_servicedesk::triage::apply(
        &mut tx,
        &ctx,
        raised_id,
        &huxley_servicedesk::triage::Answer {
            queue: &hw_name.to_uppercase(),
            priority: "p2",
            reason: "A dead laptop",
            skills: &[],
        },
    )
    .await
    .unwrap();
    let skipped = huxley_servicedesk::triage::apply(
        &mut tx,
        &ctx,
        worked_id,
        &huxley_servicedesk::triage::Answer {
            queue: &hw_name,
            priority: "p1",
            reason: "Jammed",
            skills: &[],
        },
    )
    .await
    .unwrap();
    assert!(
        huxley_servicedesk::triage::apply(
            &mut tx,
            &ctx,
            worked_id,
            &huxley_servicedesk::triage::Answer {
                queue: "Plumbing",
                priority: "p1",
                reason: "x",
                skills: &[]
            }
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    assert!(applied.applied && !skipped.applied);
    let after = s.ok(&s.owner, "GET", &url(&raised), None).await;
    assert_eq!(
        (&after["queueId"], &after["priority"]),
        (&json!(hw), &json!("p2")),
        "{after}"
    );
    let untouched = s.ok(&s.owner, "GET", &url(&worked), None).await;
    assert_eq!(untouched["queueId"], Value::Null);

    suggest_only(&s, &hw_name).await;

    // Agreement once resolved: the applied one matches, the other doesn't.
    exec(
        &s.pools,
        "UPDATE sd_tickets SET resolved_at = now() WHERE id = ANY(ARRAY[$1, $2])",
        &[raised_id, worked_id],
    )
    .await;
    let agreement = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/triage/agreement?orgId={}", s.acme),
            None,
        )
        .await;
    assert_eq!(agreement["tickets"], 2, "{agreement}");
    assert_eq!(
        (&agreement["queue"]["correct"], &agreement["queue"]["of"]),
        (&json!(1), &json!(2))
    );
    assert_eq!(agreement["applied"], 1);
}

#[tokio::test]
async fn ai_triage_accuracy_is_measured_on_labeled_samples() {
    let Some(s) = setup().await else { return };
    let ((hw, hw_name), (nw, nw_name)) = triage_queues(&s).await;
    let samples = "/api/v1/triage/samples";
    for (text, queue, priority) in [
        ("My laptop won't turn on", &hw, "p2"),
        ("Need a second monitor", &hw, "p3"),
        ("The whole office is offline", &nw, "p1"),
        ("Can I get guest Wi-Fi?", &nw, "p4"),
    ] {
        s.ok(&s.owner, "POST", samples, Some(json!({ "orgId": s.provider, "text": text, "queueId": queue, "priority": priority })))
            .await;
    }
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            samples,
            Some(json!({ "orgId": s.provider, "text": "No label" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // A resolved ticket becomes a sample, labeled with where it ended up.
    let done = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Keyboard broken", "queueId": hw, "priority": "p2" }),
        )
        .await;
    exec(
        &s.pools,
        "UPDATE sd_tickets SET resolved_at = now() WHERE id = $1",
        &[Uuid::parse_str(done["id"].as_str().unwrap()).unwrap()],
    )
    .await;
    let added = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/triage/samples/from-tickets",
            Some(json!({ "orgId": s.provider })),
        )
        .await;
    assert_eq!(added["added"], 1);
    let listed = s
        .ok(
            &s.owner,
            "GET",
            &format!("{samples}?orgId={}", s.provider),
            None,
        )
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 5);

    let evaluations = "/api/v1/triage/evaluations";
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            evaluations,
            Some(json!({ "orgId": s.provider })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "the pack isn't installed yet");
    install_triage(&s).await;
    let started = s
        .ok(
            &s.owner,
            "POST",
            evaluations,
            Some(json!({ "orgId": s.provider })),
        )
        .await;
    assert_eq!(
        (&started["total"], &started["pending"], &started["target"]),
        (&json!(5), &json!(5), &json!(80))
    );
    answer_samples(&s, started["id"].as_str().unwrap(), &hw_name, &nw_name).await;
}

/// Installs and publishes the built-in AI triage pack at the provider.
async fn install_triage(s: &Setup) {
    let review = s
        .ok(
            &s.owner,
            "POST",
            &format!(
                "/api/v1/packs/builtin/huxley/ai-triage?orgId={}",
                s.provider
            ),
            None,
        )
        .await;
    let installed = s
        .ok(
            &s.owner,
            "POST",
            &format!(
                "/api/v1/packs/uploads/{}/install",
                review["uploadId"].as_str().unwrap()
            ),
            Some(json!({ "orgId": s.provider, "version": review["version"] })),
        )
        .await;
    for id in installed["items"]["workflows"]
        .as_object()
        .unwrap()
        .values()
    {
        s.ok(
            &s.owner,
            "POST",
            &format!("/api/v1/workflows/{}/publish", id.as_str().unwrap()),
            Some(json!({})),
        )
        .await;
    }
}

/// Answers as the workflow would: right, right queue but wrong priority,
/// wrong queue, right; the last run fails. 3 of 5 queues: under the target.
async fn answer_samples(s: &Setup, evaluation: &str, hw: &str, nw: &str) {
    let evaluation_id = Uuid::parse_str(evaluation).unwrap();
    let rows: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT sample_id, run_id FROM sd_triage_results WHERE evaluation_id = $1 ORDER BY position")
            .bind(evaluation_id)
            .fetch_all(&s.pools.owner)
            .await
            .unwrap();
    assert!(
        rows.iter().all(|(_, run)| run.is_some()),
        "a run per sample"
    );
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    for ((sample, _), (queue, priority)) in
        rows.iter()
            .zip([(hw, "p2"), (hw, "p2"), (hw, "p1"), (nw, "p4")])
    {
        huxley_servicedesk::triage::answer(
            &mut tx,
            evaluation_id,
            *sample,
            queue,
            priority,
            "because",
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    exec(
        &s.pools,
        "UPDATE runs SET status = 'failed' WHERE id = $1",
        &[rows[4].1.unwrap()],
    )
    .await;
    let e = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/triage/evaluations/{evaluation}"),
            None,
        )
        .await;
    assert_eq!(
        (&e["answered"], &e["failed"], &e["pending"]),
        (&json!(4), &json!(1), &json!(0)),
        "{e}"
    );
    assert_eq!(
        (&e["queue"]["correct"], &e["queue"]["of"], &e["accuracy"]),
        (&json!(3), &json!(5), &json!(60.0))
    );
    assert_eq!(
        (&e["priority"]["correct"], &e["passed"]),
        (&json!(3), &json!(false))
    );
    let misses: Vec<(i64, bool)> = e["misses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["position"].as_i64().unwrap(), m["failed"] == true))
        .collect();
    assert_eq!(misses, [(1, false), (2, false), (4, true)]);
    let listed = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/triage/evaluations?orgId={}", s.provider),
            None,
        )
        .await;
    assert_eq!(listed[0]["id"], json!(evaluation));
}

#[tokio::test]
async fn recommendations_rank_people_with_reasons() {
    let Some(s) = setup().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "workforce", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (ada, ada_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let (bob, _) = user(&s.pools, Some(("technician", s.provider))).await;
    let (_, reader_email) = user(&s.pools, Some(("read_only", s.provider))).await;
    let queue_name = unique("Network");
    let q = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/queues",
            Some(json!({ "orgId": s.provider, "name": queue_name, "userIds": [ada, bob] })),
        )
        .await;
    let skill = Uuid::now_v7();
    exec(
        &s.pools,
        "INSERT INTO wf_skills (id, name) VALUES ($1, 'Networking ' || $1::text)",
        &[skill],
    )
    .await;
    exec(
        &s.pools,
        "INSERT INTO wf_user_skills (user_id, skill_id, level) VALUES ($1, $3, 4), ($2, $3, 1)",
        &[ada, bob, skill],
    )
    .await;
    let ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "VPN down", "queueId": q["id"] }),
        )
        .await;
    // Ada resolved two of this queue's tickets lately; Bob has two open.
    for (who, resolved) in [(ada, true), (ada, true), (bob, false), (bob, false)] {
        let t = s.ticket(&s.owner, json!({ "orgId": s.acme, "title": "Other", "queueId": q["id"], "assigneeUserId": who })).await;
        if resolved {
            exec(
                &s.pools,
                "UPDATE sd_tickets SET resolved_at = now() WHERE id = $1",
                &[Uuid::parse_str(t["id"].as_str().unwrap()).unwrap()],
            )
            .await;
        }
    }
    // A technician says what the ticket needs; a read-only user can't.
    let needs = format!(
        "/api/v1/workforce/requirements/ticket/{}",
        ticket["id"].as_str().unwrap()
    );
    let body = json!([{ "skillId": skill, "minLevel": 3 }]);
    let reader = login(&s.app, &reader_email).await;
    assert_eq!(
        s.call(&reader, "PUT", &needs, Some(body.clone())).await.0,
        StatusCode::FORBIDDEN
    );
    let tech = login(&s.app, &ada_email).await;
    s.ok(&tech, "PUT", &needs, Some(body)).await;

    ranked(&s, &tech, &ticket, (ada, bob), &queue_name).await;
}

/// Ada (skilled, with history, no load) ranks above Bob (missing the
/// skill, two open tickets), with the reasons.
async fn ranked(s: &Setup, tech: &str, ticket: &Value, (ada, bob): (Uuid, Uuid), queue_name: &str) {
    let recs = s
        .ok(
            tech,
            "GET",
            &format!("{}/recommendations", url(ticket)),
            None,
        )
        .await;
    let names: Vec<&str> = recs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["userId"].as_str().unwrap())
        .collect();
    assert_eq!(names, [ada.to_string(), bob.to_string()], "{recs}");
    let (first, second) = (&recs[0], &recs[1]);
    assert!(
        first["score"].as_i64().unwrap() > second["score"].as_i64().unwrap(),
        "{recs}"
    );
    let reasons: Vec<&str> = first["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert!(
        reasons.contains(&"Meets every skill level it needs"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&format!("Resolved 2 in {queue_name} lately").as_str()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"0 open tickets (the lightest load)"),
        "{reasons:?}"
    );
    assert_eq!(second["openTickets"], 2);
    assert!(
        second["cautions"][0]
            .as_str()
            .unwrap()
            .starts_with("Missing: Networking"),
        "{second}"
    );
}

/// With `triage.auto_apply` off at Globex, suggestions are only recorded.
async fn suggest_only(s: &Setup, hw_name: &str) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_orgs::settings::set(
        &mut tx,
        &ctx,
        s.globex,
        huxley_orgs::settings::TRIAGE_AUTO,
        "false",
        false,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.globex, "title": "Mouse broken" }),
        )
        .await;
    let id = Uuid::parse_str(ticket["id"].as_str().unwrap()).unwrap();
    exec(
        &s.pools,
        "UPDATE sd_ticket_activity SET actor_id = NULL WHERE ticket_id = $1",
        &[id],
    )
    .await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let skills = vec!["Hardware repair".to_owned()];
    let answer = huxley_servicedesk::triage::Answer {
        queue: hw_name,
        priority: "p2",
        reason: "A broken mouse",
        skills: &skills,
    };
    let out = huxley_servicedesk::triage::apply(&mut tx, &ctx, id, &answer)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        !out.applied && !out.apply_skills && out.suggest_only,
        "{out:?}"
    );
    let after = s.ok(&s.owner, "GET", &url(&ticket), None).await;
    assert_eq!(after["queueId"], Value::Null);
    let kept: Vec<String> =
        sqlx::query_scalar("SELECT unnest(skills) FROM sd_triage WHERE ticket_id = $1")
            .bind(id)
            .fetch_all(&s.pools.owner)
            .await
            .unwrap();
    assert_eq!(kept, skills);
}

#[tokio::test]
async fn tickets_show_their_insight_and_similar_tickets() {
    let Some(s) = setup().await else { return };
    let printer = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Printer offline" }),
        )
        .await;
    let toner = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Toner light on" }),
        )
        .await;
    let id = Uuid::parse_str(printer["id"].as_str().unwrap()).unwrap();
    let insight = format!("{}/insight", url(&printer));
    assert_eq!(s.ok(&s.owner, "GET", &insight, None).await, Value::Null);
    // Refresh asks the pack, through an event.
    let (status, _) = s
        .call(&s.owner, "POST", &format!("{insight}/refresh"), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let asked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events_outbox WHERE event_type = 'ticket.insight_requested' AND subject_id = $1",
    )
    .bind(id)
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(asked, 1);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_servicedesk::insights::record(
        &mut tx,
        id,
        "The second-floor printer is offline.",
        &json!({ "devices": ["PRN-2F"], "people": [] }),
        Some("Power-cycle it."),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let shown = s.ok(&s.owner, "GET", &insight, None).await;
    assert_eq!(
        (&shown["summary"], &shown["nextStep"]),
        (
            &json!("The second-floor printer is offline."),
            &json!("Power-cycle it.")
        )
    );
    assert_eq!(shown["entities"]["devices"], json!(["PRN-2F"]));

    // Similar tickets come from the ticket's own embedding.
    let similar = format!("{}/similar", url(&printer));
    let found = s.ok(&s.owner, "GET", &similar, None).await;
    assert_eq!(found["available"], false, "not embedded yet: {found}");
    let pgvector: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    if pgvector {
        // A model of its own: other runs' embeddings don't count.
        let model = format!("test-{}", Uuid::now_v7());
        for (ticket, v) in [(&printer, [1.0f32, 0.0, 0.0]), (&toner, [0.9, 0.1, 0.0])] {
            sqlx::query(
                "INSERT INTO sd_ticket_embeddings (ticket_id, org_id, model, content_hash, embedding) VALUES ($1, $2, $4, 'x', $3)",
            )
            .bind(Uuid::parse_str(ticket["id"].as_str().unwrap()).unwrap())
            .bind(s.acme)
            .bind(v.to_vec())
            .bind(&model)
            .execute(&s.pools.owner)
            .await
            .unwrap();
        }
        let found = s.ok(&s.owner, "GET", &similar, None).await;
        assert_eq!(
            (&found["available"], &found["tickets"][0]["id"]),
            (&json!(true), &toner["id"]),
            "{found}"
        );
    } else {
        assert!(
            found["message"].as_str().unwrap().contains("pgvector"),
            "{found}"
        );
    }
}

#[tokio::test]
async fn suggested_time_is_accepted_or_dismissed_by_its_person() {
    let Some(s) = setup().await else { return };
    let (tech, tech_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let tech_cookie = login(&s.app, &tech_email).await;
    let other = login(
        &s.app,
        &user(&s.pools, Some(("technician", s.provider))).await.1,
    )
    .await;
    let first = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Mailbox full", "assigneeUserId": tech }),
        )
        .await;
    let second = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "Mouse dead", "assigneeUserId": tech }),
        )
        .await;
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    for t in [&first, &second] {
        let id = Uuid::parse_str(t["id"].as_str().unwrap()).unwrap();
        let made = huxley_servicedesk::suggestions::suggest(&mut tx, id, None, 30, "Cleared it")
            .await
            .unwrap();
        assert!(made.is_some());
        // Waiting already: not suggested twice.
        assert!(
            huxley_servicedesk::suggestions::suggest(&mut tx, id, None, 30, "x")
                .await
                .unwrap()
                .is_none()
        );
    }
    tx.commit().await.unwrap();
    let waiting = s
        .ok(&tech_cookie, "GET", "/api/v1/time-suggestions", None)
        .await;
    assert_eq!(waiting.as_array().unwrap().len(), 2, "{waiting}");
    assert_eq!(
        s.ok(&other, "GET", "/api/v1/time-suggestions", None).await,
        json!([])
    );
    let work_type = s.ok(&s.owner, "GET", "/api/v1/work-types", None).await[0]["id"].clone();
    let (a, b) = (
        waiting[0]["id"].as_str().unwrap(),
        waiting[1]["id"].as_str().unwrap(),
    );
    // Someone else can't take yours.
    let (status, _) = s
        .call(
            &other,
            "POST",
            &format!("/api/v1/time-suggestions/{a}/accept"),
            Some(json!({ "workTypeId": work_type })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let entry = s
        .ok(
            &tech_cookie,
            "POST",
            &format!("/api/v1/time-suggestions/{a}/accept"),
            Some(json!({ "workTypeId": work_type, "minutes": 40 })),
        )
        .await;
    assert_eq!(
        (&entry["minutes"], &entry["notes"]),
        (&json!(40), &json!("Cleared it")),
        "{entry}"
    );
    let (status, _) = s
        .call(
            &tech_cookie,
            "POST",
            &format!("/api/v1/time-suggestions/{b}/dismiss"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        s.ok(&tech_cookie, "GET", "/api/v1/time-suggestions", None)
            .await,
        json!([])
    );
    // Time logged now: no more suggestions for that ticket.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let id = Uuid::parse_str(entry["ticketId"].as_str().unwrap()).unwrap();
    assert!(
        huxley_servicedesk::suggestions::suggest(&mut tx, id, None, 30, "again")
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

/// A server at Acme and an app that runs on it: `(server, app)` ids.
async fn server_and_app(s: &Setup) -> (String, String) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "cmdb", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let kind = s.ok(&s.owner, "GET", "/api/v1/asset-types", None).await[0]["id"].clone();
    let mut ids = Vec::new();
    for name in ["SRV-01", "Payroll app"] {
        let a = s
            .ok(
                &s.owner,
                "POST",
                "/api/v1/assets",
                Some(json!({ "orgId": s.acme, "typeId": kind, "name": unique(name) })),
            )
            .await;
        ids.push(a["id"].as_str().unwrap().to_owned());
    }
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/assets/{}/relationships", ids[1]),
        Some(json!({ "kind": "runs_on", "toId": ids[0] })),
    )
    .await;
    (ids.remove(0), ids.remove(0))
}

const HIGH: &str = r#"{ "reach": "everyone", "downtime": "hours", "tested": "partly", "backout": "hard", "familiar": "once" }"#;

#[tokio::test]
async fn changes_are_assessed_submitted_and_kept_out_of_freezes() {
    let Some(s) = setup().await else { return };
    let (server, app) = server_and_app(&s).await;
    let ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "type": "change", "title": "Patch SRV-01" }),
        )
        .await;
    let change = format!("{}/change", url(&ticket));
    let fresh = s.ok(&s.owner, "GET", &change, None).await;
    assert_eq!(
        (&fresh["state"], &fresh["changeType"]),
        (&json!("draft"), &json!("normal"))
    );
    // Not a change ticket: refused.
    let incident = s
        .ticket(&s.owner, json!({ "orgId": s.acme, "title": "Broken" }))
        .await;
    assert_eq!(
        s.call(&s.owner, "GET", &format!("{}/change", url(&incident)), None)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let start = time::OffsetDateTime::now_utc() + time::Duration::days(10);
    let iso = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let saved = s
        .ok(
            &s.owner,
            "PUT",
            &change,
            Some(json!({
                "riskAnswers": serde_json::from_str::<Value>(HIGH).unwrap(),
                "implementationPlan": "Patch and reboot.", "backoutPlan": "Restore the snapshot.",
                "plannedStart": iso(start), "plannedEnd": iso(start + time::Duration::hours(2)),
                "affectedAssets": [server], "version": fresh["version"],
            })),
        )
        .await;
    assert_eq!(
        (&saved["riskLevel"], &saved["riskScore"]),
        (&json!("high"), &json!(10)),
        "{saved}"
    );
    let impact = s
        .ok(&s.owner, "GET", &format!("{change}/impact"), None)
        .await;
    assert_eq!(impact["affected"][0]["id"], json!(server));
    assert_eq!(impact["impacted"][0]["id"], json!(app), "{impact}");

    // A freeze over the window at the MSP blocks it; an emergency goes through.
    let freeze = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/change-freezes",
            Some(json!({ "orgId": s.provider, "name": "Year end", "startsAt": iso(start - time::Duration::days(1)),
                         "endsAt": iso(start + time::Duration::days(1)) })),
        )
        .await;
    let (status, problem) = s
        .call(&s.owner, "POST", &format!("{change}/submit"), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        problem["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Year end"),
        "{problem}"
    );
    let calendar = s
        .ok(
            &s.owner,
            "GET",
            &format!(
                "/api/v1/changes/calendar?from={}",
                urlencode(&iso(start - time::Duration::days(2)))
            ),
            None,
        )
        .await;
    assert!(
        calendar["freezes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == freeze["id"]),
        "{calendar}"
    );
    s.ok(
        &s.owner,
        "DELETE",
        &format!("/api/v1/change-freezes/{}", freeze["id"].as_str().unwrap()),
        None,
    )
    .await;
    change_lifecycle(&s, &ticket, &change).await;
}

fn urlencode(s: &str) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}

/// Submitted, decided (as the approvals pack would), implemented, reviewed.
async fn change_lifecycle(s: &Setup, ticket: &Value, change: &str) {
    let submitted = s
        .ok(&s.owner, "POST", &format!("{change}/submit"), None)
        .await;
    assert_eq!(submitted["change"]["state"], "submitted", "{submitted}");
    let id = Uuid::parse_str(ticket["id"].as_str().unwrap()).unwrap();
    let event: Value = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'change.submitted' AND subject_id = $1",
    )
    .bind(id)
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(
        (&event["riskLevel"], &event["changeType"]),
        (&json!("high"), &json!("normal"))
    );
    // Submitted: it can't be changed until decided.
    assert_eq!(
        s.call(&s.owner, "PUT", change, Some(json!({ "testPlan": "x" })))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let ctx = RequestContext::system(Via::Workflow);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_servicedesk::changes::decide(&mut tx, &ctx, id, true, "Fay (client)", Some("Go ahead"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let approved = s.ok(&s.owner, "GET", change, None).await;
    assert_eq!(
        (&approved["state"], &approved["decidedBy"]),
        (&json!("approved"), &json!("Fay (client)"))
    );
    let (status, _) = s
        .call(
            &s.owner,
            "POST",
            &format!("{change}/review"),
            Some(json!({ "review": "Too early" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    s.ok(
        &s.owner,
        "POST",
        &format!("{change}/implemented"),
        Some(json!({ "outcome": "successful" })),
    )
    .await;
    let reviewed = s
        .ok(
            &s.owner,
            "POST",
            &format!("{change}/review"),
            Some(json!({ "review": "Went to plan." })),
        )
        .await;
    assert_eq!(
        (&reviewed["state"], &reviewed["outcome"]),
        (&json!("reviewed"), &json!("successful"))
    );
}

#[tokio::test]
async fn standard_changes_are_pre_approved() {
    let Some(s) = setup().await else { return };
    let ticket = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "type": "change", "title": "Add a mailbox" }),
        )
        .await;
    let change = format!("{}/change", url(&ticket));
    let start = time::OffsetDateTime::now_utc() + time::Duration::days(3);
    let iso = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    let low = json!({ "reach": "nobody", "downtime": "none", "tested": "yes", "backout": "easy", "familiar": "often" });
    s.ok(
        &s.owner,
        "PUT",
        &change,
        Some(json!({ "changeType": "standard", "riskAnswers": low,
        "plannedStart": iso(start), "plannedEnd": iso(start + time::Duration::minutes(30)) })),
    )
    .await;
    let done = s
        .ok(&s.owner, "POST", &format!("{change}/submit"), None)
        .await;
    assert_eq!(
        (&done["change"]["state"], &done["change"]["riskLevel"]),
        (&json!("approved"), &json!("low")),
        "{done}"
    );
    let questions = s
        .ok(&s.owner, "GET", "/api/v1/changes/questions", None)
        .await;
    assert_eq!(questions.as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn problems_link_incidents_become_known_errors_and_draft_articles() {
    let Some(s) = setup().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "knowledge", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let problem = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "type": "problem", "title": "VPN drops hourly" }),
        )
        .await;
    let incident = s
        .ticket(
            &s.owner,
            json!({ "orgId": s.acme, "title": "VPN dropped again" }),
        )
        .await;
    // An incident linked as caused by the problem; the problem lists it.
    s.ok(
        &s.owner,
        "POST",
        &format!("{}/links", url(&problem)),
        Some(json!({ "otherId": incident["id"], "kind": "incident" })),
    )
    .await;
    let from_incident = s
        .ok(&s.owner, "GET", &format!("{}/links", url(&incident)), None)
        .await;
    assert_eq!(
        (&from_incident[0]["kind"], &from_incident[0]["id"]),
        (&json!("problem"), &problem["id"]),
        "{from_incident}"
    );
    let record = format!("{}/problem", url(&problem));
    assert_eq!(s.ok(&s.owner, "GET", &record, None).await["incidents"], 1);
    // A known error needs the cause and a workaround.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &record,
            Some(json!({ "knownError": true })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let known = s
        .ok(&s.owner, "PUT", &record, Some(json!({ "rootCause": "The firewall drops idle tunnels.", "workaround": "Reconnect.", "knownError": true })))
        .await;
    assert!(
        known["knownError"] == true && known["knownErrorAt"].is_string(),
        "{known}"
    );
    let id = Uuid::parse_str(problem["id"].as_str().unwrap()).unwrap();
    let raised: i64 = sqlx::query_scalar("SELECT count(*) FROM events_outbox WHERE event_type = 'problem.known_error' AND subject_id = $1")
        .bind(id)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(raised, 1);
    // The workaround, drafted into the knowledge base for review.
    let drafted = s
        .ok(&s.owner, "POST", &format!("{record}/article"), None)
        .await;
    let (status, tags, title): (String, Vec<String>, String) =
        sqlx::query_as("SELECT status, tags, title FROM kb_articles WHERE id = $1")
            .bind(Uuid::parse_str(drafted["articleId"].as_str().unwrap()).unwrap())
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), tags, title.as_str()),
        (
            "in_review",
            vec!["known-error".to_owned()],
            "Known error: VPN drops hourly"
        )
    );
    assert_eq!(
        s.ok(&s.owner, "GET", &record, None).await["kbArticleId"],
        drafted["articleId"]
    );
    // Only problem tickets have problem details.
    assert_eq!(
        s.call(
            &s.owner,
            "GET",
            &format!("{}/problem", url(&incident)),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

/// An onboarding template at the provider: two phases (4 and 6 budget
/// hours), three tasks (b waits on a; c is for the client) and two
/// milestones.
async fn onboarding_template(s: &Setup) -> Value {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "projects", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    s.ok(
        &s.owner,
        "POST",
        "/api/v1/project-templates",
        Some(json!({
            "orgId": s.provider, "name": unique("Client onboarding"),
            "structure": {
                "phases": [
                    { "name": "Discovery", "budgetHours": 4, "tasks": [
                        { "key": "audit", "title": "Audit the network", "estimateHours": 2 },
                        { "key": "plan", "title": "Write the plan", "estimateHours": 2, "dependsOn": ["audit"] } ] },
                    { "name": "Rollout", "budgetHours": 6, "tasks": [
                        { "key": "deploy", "title": "Deploy the agents", "estimateHours": 4, "clientVisible": true } ] }
                ],
                "milestones": [ { "name": "Kickoff", "offsetDays": 0 }, { "name": "Live", "offsetDays": 14 } ]
            }
        })),
    )
    .await
}

#[tokio::test]
async fn projects_from_templates_assign_tasks_and_track_budget() {
    let Some(s) = setup().await else { return };
    let template = onboarding_template(&s).await;
    let (ada, ada_email) = user(&s.pools, Some(("technician", s.provider))).await;
    let (bob, _) = user(&s.pools, Some(("technician", s.provider))).await;
    let ada_cookie = login(&s.app, &ada_email).await;
    // Technicians work projects; they don't create them.
    let (status, _) = s
        .call(
            &ada_cookie,
            "POST",
            "/api/v1/projects",
            Some(json!({ "orgId": s.acme, "name": "Nope", "startOn": "2026-12-01" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            Some(json!({ "orgId": s.acme, "name": "Acme onboarding", "startOn": "2026-12-01", "templateId": template["id"],
                         "assignees": { "audit": ada, "deploy": bob }, "thresholds": [50, 100] })),
        )
        .await;
    assert_eq!(
        (&project["budgetHours"], &project["progress"]["total"]),
        (&json!(10.0), &json!(3)),
        "{project}"
    );
    assert_eq!(project["milestones"][1]["dueOn"], "2026-12-15");
    let tasks = project["tasks"].as_array().unwrap();
    let task = |title: &str| tasks.iter().find(|t| t["title"] == title).unwrap().clone();
    assert_eq!(task("Audit the network")["assigneeUserId"], json!(ada));
    assert_eq!(
        task("Write the plan")["waiting"],
        true,
        "it waits on the audit"
    );
    // The task is in Ada's My Work like any ticket.
    let work = s.ok(&ada_cookie, "GET", "/api/v1/my-work", None).await;
    let audit = task("Audit the network");
    assert!(
        work["mine"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == audit["ticketId"]),
        "{work}"
    );
    project_budget(&s, &project, &audit, &ada_cookie).await;
}

/// Time on a task burns the budget; each threshold fires once; the portal
/// shows milestones and client-facing tasks.
async fn project_budget(s: &Setup, project: &Value, audit: &Value, ada: &str) {
    let id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let remote = work_type(s, "Remote support").await;
    let time = format!(
        "/api/v1/tickets/{}/time",
        audit["ticketId"].as_str().unwrap()
    );
    for minutes in [300, 60] {
        s.ok(
            ada,
            "POST",
            &time,
            Some(json!({ "workTypeId": remote["id"], "minutes": minutes })),
        )
        .await;
    }
    let fired = || async {
        sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM events_outbox WHERE event_type = 'project.budget_threshold' AND subject_id = $1 ORDER BY occurred_at, id",
        )
        .bind(id)
        .fetch_all(&s.pools.owner)
        .await
        .unwrap()
    };
    let events = fired().await;
    assert_eq!(events.len(), 1, "50% once: {events:?}");
    assert_eq!(
        (&events[0]["threshold"], &events[0]["percent"]),
        (&json!(50), &json!(50.0))
    );
    s.ok(
        ada,
        "POST",
        &time,
        Some(json!({ "workTypeId": remote["id"], "minutes": 300 })),
    )
    .await;
    assert_eq!(fired().await.len(), 2, "then 100%");
    let url = format!("/api/v1/projects/{id}");
    let now = s.ok(&s.owner, "GET", &url, None).await;
    assert_eq!(
        (
            &now["usedHours"],
            &now["burnPercent"],
            &now["health"]["level"]
        ),
        (&json!(11.0), &json!(110.0), &json!("off_track"))
    );
    // Reaching a milestone.
    let kickoff = now["milestones"][0]["id"].as_str().unwrap();
    s.ok(
        ada,
        "POST",
        &format!("{url}/milestones/{kickoff}"),
        Some(json!({ "reached": true })),
    )
    .await;
    let reached: i64 = sqlx::query_scalar("SELECT count(*) FROM events_outbox WHERE event_type = 'project.milestone_reached' AND subject_id = $1")
        .bind(id)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(reached, 1);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let portal = huxley_servicedesk::projects::for_portal(&mut tx, s.acme)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mine = portal.iter().find(|p| p.id == id).unwrap();
    assert_eq!(mine.milestones.len(), 2);
    assert!(mine.milestones[0].reached_at.is_some());
    assert_eq!(
        mine.tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        ["Deploy the agents"]
    );
}

#[tokio::test]
async fn scope_changes_are_submitted_decided_and_grow_the_budget() {
    let Some(s) = setup().await else { return };
    let template = onboarding_template(&s).await;
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            Some(json!({ "orgId": s.acme, "name": "Acme scope", "startOn": "2026-12-01", "templateId": template["id"] })),
        )
        .await;
    let url = format!("/api/v1/projects/{}", project["id"].as_str().unwrap());
    // Task dates: a due date before the start is refused.
    let task = project["tasks"][0]["ticketId"].as_str().unwrap();
    let (status, _) = s
        .call(
            &s.owner,
            "PATCH",
            &format!("{url}/tasks/{task}"),
            Some(json!({ "startOn": "2026-12-05", "dueOn": "2026-12-04" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let dated = s
        .ok(
            &s.owner,
            "PATCH",
            &format!("{url}/tasks/{task}"),
            Some(json!({ "startOn": "2026-12-02", "dueOn": "2026-12-04" })),
        )
        .await;
    assert_eq!(
        (&dated["tasks"][0]["startOn"], &dated["tasks"][0]["dueOn"]),
        (&json!("2026-12-02"), &json!("2026-12-04"))
    );

    let change = s
        .ok(
            &s.owner,
            "POST",
            &format!("{url}/scope-changes"),
            Some(json!({ "title": "Add a second site", "hours": 6, "amount": 900 })),
        )
        .await;
    assert_eq!(
        (&change["number"], &change["state"]),
        (&json!(1), &json!("draft"))
    );
    let at = format!("{url}/scope-changes/{}", change["id"].as_str().unwrap());
    let submitted = s.ok(&s.owner, "POST", &format!("{at}/submit"), None).await;
    assert_eq!(submitted["state"], "submitted");
    let event: Value = sqlx::query_scalar(
        "SELECT payload FROM events_outbox WHERE event_type = 'project.scope_change_submitted' AND subject_id = $1",
    )
    .bind(Uuid::parse_str(project["id"].as_str().unwrap()).unwrap())
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(
        (&event["title"], &event["hours"], &event["templateId"]),
        (&json!("Add a second site"), &json!(6.0), &template["id"])
    );
    // Submitted changes aren't edited.
    let (status, _) = s
        .call(
            &s.owner,
            "PUT",
            &at,
            Some(json!({ "title": "Sneaky", "hours": 60 })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let ctx = RequestContext::system(Via::Workflow);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let id = Uuid::parse_str(change["id"].as_str().unwrap()).unwrap();
    huxley_servicedesk::project_scope::decide(
        &mut tx,
        &ctx,
        id,
        true,
        "Cleo (client)",
        Some("Fine"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let now = s.ok(&s.owner, "GET", &url, None).await;
    assert_eq!(now["budgetHours"], json!(16.0), "10 + 6");
    assert_eq!(
        (
            &now["scopeChanges"][0]["state"],
            &now["scopeChanges"][0]["decidedBy"]
        ),
        (&json!("approved"), &json!("Cleo (client)"))
    );
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let portal = huxley_servicedesk::projects::for_portal(&mut tx, s.acme)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mine = portal.iter().find(|p| p.name == "Acme scope").unwrap();
    assert_eq!(mine.scope_changes.len(), 1);

    template_workflow_is_listed(&s, &template).await;
}

/// A workflow filtered on a template is one of its workflows.
async fn template_workflow_is_listed(s: &Setup, template: &Value) {
    let tid = template["id"].as_str().unwrap();
    let wf = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/workflows",
            Some(json!({ "orgId": s.provider, "document": {
                "apiVersion": "usehuxley.io/v1", "kind": "Workflow",
                "metadata": { "key": unique("kickoff").replace(' ', "-").to_lowercase(), "name": "Kickoff" },
                "spec": {
                    "triggers": [{ "type": "event", "event": "project.created",
                                   "filter": format!("{{{{ event.payload.templateId == '{tid}' }}}}") }],
                    "steps": [{ "id": "hi", "type": "transform", "output": "hi" }]
                }
            } })),
        )
        .await;
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/workflows/{}/publish", wf["id"].as_str().unwrap()),
        Some(json!({})),
    )
    .await;
    let listed = s
        .ok(
            &s.owner,
            "GET",
            &format!("/api/v1/project-templates/{tid}/workflows"),
            None,
        )
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(
        (&listed[0]["name"], &listed[0]["event"]),
        (&json!("Kickoff"), &json!("project.created"))
    );
}

#[tokio::test]
async fn capacity_spreads_task_estimates_and_flags_over_booking() {
    let Some(s) = setup().await else { return };
    let skill = Uuid::now_v7();
    sqlx::query("INSERT INTO wf_skills (id, name, category) VALUES ($1, $2, 'Networking')")
        .bind(skill)
        .bind(unique("Firewalls"))
        .execute(&s.pools.owner)
        .await
        .unwrap();
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    huxley_modules::set_enabled(&mut tx, &ctx, "projects", true, false)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let template = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/project-templates",
            Some(json!({ "orgId": s.provider, "name": unique("Firewall swap"), "structure": { "phases": [
                { "name": "Swap", "tasks": [{ "key": "fw", "title": "Swap the firewall", "estimateHours": 8,
                                              "startOffsetDays": 0, "durationDays": 4,
                                              "skills": [{ "skillId": skill, "minLevel": 3 }] }] } ] } })),
        )
        .await;
    let (ada, _) = user(&s.pools, Some(("technician", s.provider))).await;
    // Monday 7 January 2030: the task is spread over that week.
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            Some(json!({ "orgId": s.acme, "name": "Firewall", "startOn": "2030-01-07", "templateId": template["id"],
                         "assignees": { "fw": ada } })),
        )
        .await;
    let task = &project["tasks"][0];
    assert_eq!(
        (&task["startOn"], &task["dueOn"]),
        (&json!("2030-01-07"), &json!("2030-01-11"))
    );
    let needs: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_requirements WHERE target_type = 'ticket' AND target_id = $1 AND skill_id = $2")
        .bind(Uuid::parse_str(task["ticketId"].as_str().unwrap()).unwrap())
        .bind(skill)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(needs, 1, "the template's skill is the task's requirement");

    let url = "/api/v1/projects/capacity?from=2030-01-09&weeks=2";
    let mine = |plan: &Value| {
        plan["people"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["userId"] == json!(ada))
            .unwrap()
            .clone()
    };
    let plan = s.ok(&s.owner, "GET", url, None).await;
    assert_eq!(plan["weeks"], json!(["2030-01-07", "2030-01-14"]));
    let week = &mine(&plan)["weeks"][0];
    assert_eq!(
        (&week["capacityHours"], &week["projectHours"], &week["over"]),
        (&json!(40.0), &json!(8.0), &json!(false))
    );
    // Another 36 hours that week books Ada beyond 40.
    let id = project["id"].as_str().unwrap();
    s.ok(
        &s.owner,
        "POST",
        &format!("/api/v1/projects/{id}/tasks"),
        Some(json!({ "title": "Cabling", "assigneeUserId": ada, "estimateHours": 36, "startOn": "2030-01-07", "dueOn": "2030-01-11" })),
    )
    .await;
    let plan = s.ok(&s.owner, "GET", url, None).await;
    let me = mine(&plan);
    assert_eq!(
        (
            &me["weeks"][0]["bookedHours"],
            &me["weeks"][0]["over"],
            &me["openTasks"]
        ),
        (&json!(44.0), &json!(true), &json!(2))
    );
    assert_eq!(me["weeks"][1]["projectHours"], json!(0.0));
}

#[tokio::test]
async fn status_reports_are_written_shared_and_shown_in_the_portal() {
    let Some(s) = setup().await else { return };
    onboarding_template(&s).await; // turns projects on
    let project = s
        .ok(
            &s.owner,
            "POST",
            "/api/v1/projects",
            Some(json!({ "orgId": s.acme, "name": "Acme reports", "startOn": "2026-12-01" })),
        )
        .await;
    let url = format!(
        "/api/v1/projects/{}/status-reports",
        project["id"].as_str().unwrap()
    );
    let report = s
        .ok(
            &s.owner,
            "POST",
            &url,
            Some(json!({ "body": "All good this week." })),
        )
        .await;
    assert_eq!(
        (&report["clientVisible"], &report["health"]),
        (&json!(false), &json!("on_track"))
    );
    let portal = |s: &Setup| {
        let pools = s.pools.app.clone();
        let acme = s.acme;
        async move {
            let mut tx = begin_scoped(&pools, &OrgScope::System).await.unwrap();
            let all = huxley_servicedesk::projects::for_portal(&mut tx, acme)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            all.into_iter().find(|p| p.name == "Acme reports").unwrap()
        }
    };
    assert!(portal(&s).await.latest_report.is_none(), "not shared yet");
    let at = format!("{url}/{}", report["id"].as_str().unwrap());
    s.ok(
        &s.owner,
        "PATCH",
        &at,
        Some(json!({ "clientVisible": true })),
    )
    .await;
    assert_eq!(
        portal(&s).await.latest_report.unwrap().body,
        "All good this week."
    );
    let (status, _) = s.call(&s.owner, "DELETE", &at, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(portal(&s).await.latest_report.is_none());
}
