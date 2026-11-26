//! The semantic data layer's API (ADR-041) against a real PostgreSQL:
//! numbers cover only the organizations where you can read the source.

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use huxley_api::{ApiConfig, AppState, router};
use huxley_auth::{AuthConfig, AuthService};
use huxley_core::{RequestContext, Via};
use huxley_crypto::Crypto;
use huxley_db::testing::{TestPools, ensure_root, isolated, unique};
use huxley_db::{OrgScope, begin_scoped};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const PASSWORD: &str = "analytics api passphrase";

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

/// A user, optionally with a built-in role at an organization.
async fn user(pools: &TestPools, role_at: Option<(&str, Uuid)>) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("analytics").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Analytics User", PASSWORD)
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

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-requested-with", "huxley")
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn app_for(pools: &TestPools) -> axum::Router {
    let crypto = || Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto());
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(pools.app.clone(), auth, crypto(), config))
}

#[tokio::test]
async fn numbers_cover_only_what_you_can_read() {
    let Some(db) = isolated("analytics_api").await else {
        return;
    };
    let pools = db.pools.clone();
    let app = app_for(&pools);
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let acme = org(&pools, root, huxley_orgs::OrgKind::Client, "Acme").await;
    let globex = org(&pools, root, huxley_orgs::OrgKind::Client, "Globex").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    for (org, n) in [(acme, 2), (globex, 3)] {
        for i in 0..n {
            let (status, t) = call(
                &app,
                &owner,
                "POST",
                "/api/v1/tickets",
                json!({ "orgId": org, "title": format!("Ticket {i}") }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{t}");
        }
    }
    let by_client = json!({
        "source": "tickets", "measures": [{ "fn": "count" }], "dimensions": [{ "field": "client" }],
        "filters": [{ "field": "open", "op": "eq", "value": true }],
    });

    // The owner sees both clients, biggest first.
    let (status, answer) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/analytics/query",
        by_client.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let rows = answer["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{answer}");
    assert_eq!(rows[0][1], 3);

    // A read-only user at Acme sees Acme's numbers only.
    let reader = login(&app, &user(&pools, Some(("read_only", acme))).await.1).await;
    let (_, catalog) = call(
        &app,
        &reader,
        "GET",
        "/api/v1/analytics/sources",
        Value::Null,
    )
    .await;
    let keys: Vec<&str> = catalog["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"tickets"), "{catalog}");
    assert!(
        catalog["ranges"]
            .as_array()
            .unwrap()
            .contains(&json!("last_30_days"))
    );
    let (status, answer) = call(&app, &reader, "POST", "/api/v1/analytics/query", by_client).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["rows"],
        json!([[answer["rows"][0][0], 2]]),
        "{answer}"
    );

    // Bad specs point at what's wrong.
    let (status, problem) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/analytics/query",
        json!({ "source": "tickets", "measures": [{ "fn": "sum", "field": "priority" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        problem["errors"][0]["pointer"], "/measures/0/field",
        "{problem}"
    );
}

#[tokio::test]
async fn dashboards_are_prebuilt_shared_or_personal() {
    let Some(db) = isolated("dashboards_api").await else {
        return;
    };
    let pools = db.pools.clone();
    let app = app_for(&pools);
    let root = ensure_root(&pools).await;
    let acme = org(&pools, root, huxley_orgs::OrgKind::Client, "Acme").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    let reader = login(&app, &user(&pools, Some(("read_only", acme))).await.1).await;
    let widget = json!({ "id": "n", "title": "Runs", "viz": "number", "width": 3,
                         "spec": { "source": "runs", "measures": [{ "fn": "count" }] } });

    let (_, all) = call(&app, &reader, "GET", "/api/v1/dashboards", Value::Null).await;
    let builtin: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["builtin"] == true)
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        builtin.join(","),
        "service-desk,csat,nps,sla,utilization,clients,time"
    );
    let (status, sd) = call(
        &app,
        &reader,
        "GET",
        "/api/v1/dashboards/service-desk",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sd["widgets"][0]["id"], "open");

    // The owner's personal dashboard is theirs alone; a shared one at Acme is Acme's people's.
    let create = |org: Uuid, shared: bool, name: &str| json!({ "orgId": org, "shared": shared, "dashboard": { "name": name, "widgets": [widget] } });
    let (status, mine) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/dashboards",
        create(root, false, "Mine"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    let (status, shared) = call(
        &app,
        &owner,
        "POST",
        "/api/v1/dashboards",
        create(acme, true, "Acme board"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{shared}");
    let (_, seen) = call(&app, &reader, "GET", "/api/v1/dashboards", Value::Null).await;
    let names: Vec<&str> = seen
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["builtin"] == false)
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Acme board"]);
    let mine_uri = format!("/api/v1/dashboards/{}", mine["id"].as_str().unwrap());
    assert_eq!(
        call(&app, &reader, "GET", &mine_uri, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );

    // Readers keep personal dashboards but don't change shared ones.
    let shared_uri = format!("/api/v1/dashboards/{}", shared["id"].as_str().unwrap());
    let (status, _) = call(
        &app,
        &reader,
        "PUT",
        &shared_uri,
        json!({ "name": "Renamed", "widgets": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &app,
        &reader,
        "POST",
        "/api/v1/dashboards",
        create(acme, false, "My Acme"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        &app,
        &reader,
        "POST",
        "/api/v1/dashboards",
        create(acme, true, "Not allowed"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    edits_and_refusals(&app, &owner, &shared_uri, &widget).await;
}

/// The owner changes a shared dashboard (with the version); stale versions
/// and bad widgets are refused; it's deleted.
async fn edits_and_refusals(app: &axum::Router, owner: &str, shared_uri: &str, widget: &Value) {
    // The owner changes the shared one (with the version), and bad widgets are refused.
    let (status, renamed) = call(
        app,
        owner,
        "PUT",
        shared_uri,
        json!({ "name": "Acme overview", "widgets": [widget], "version": 1 }),
    )
    .await;
    assert_eq!(
        (status, &renamed["version"]),
        (StatusCode::OK, &json!(2)),
        "{renamed}"
    );
    let (status, _) = call(
        app,
        owner,
        "PUT",
        shared_uri,
        json!({ "name": "Stale", "widgets": [], "version": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let pie = json!({ "id": "p", "title": "Pie", "viz": "pie", "spec": { "source": "runs", "measures": [{ "fn": "count" }] } });
    let (status, problem) = call(
        app,
        owner,
        "PUT",
        shared_uri,
        json!({ "name": "Bad", "widgets": [pie] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        problem["errors"][0]["pointer"], "/widgets/0/viz",
        "{problem}"
    );
    assert_eq!(
        call(app, owner, "DELETE", shared_uri, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn datasets_are_sources_too() {
    let Some(db) = isolated("analytics_datasets").await else {
        return;
    };
    let pools = db.pools.clone();
    let app = app_for(&pools);
    let root = ensure_root(&pools).await;
    let acme = org(&pools, root, huxley_orgs::OrgKind::Client, "Acme").await;
    let globex = org(&pools, root, huxley_orgs::OrgKind::Client, "Globex").await;
    let owner = login(&app, &user(&pools, None).await.1).await;
    let (status, ds) = call(&app, &owner, "POST", "/api/v1/datasets", json!({ "orgId": root, "dataset": {
        "key": "computers", "name": "Computers",
        "columns": [{ "name": "hostname", "type": "text" }, { "name": "ram_gb", "type": "number" },
                    { "name": "last_seen", "type": "timestamp" }, { "name": "raw", "type": "json" }],
    } })).await;
    assert_eq!(status, StatusCode::CREATED, "{ds}");
    let rows = format!("/api/v1/datasets/{}/rows", ds["id"].as_str().unwrap());
    for (org, ram) in [(acme, [8, 16]), (globex, [32, 32])] {
        let (status, written) = call(
            &app,
            &owner,
            "POST",
            &rows,
            json!({ "orgId": org, "rows": [
            { "hostname": "a", "ram_gb": ram[0], "last_seen": "2026-11-20T10:00:00Z" },
            { "hostname": "b", "ram_gb": ram[1], "last_seen": "2026-11-21T10:00:00Z" },
        ] }),
        )
        .await;
        assert!(status.is_success(), "{written}");
    }

    // A read-only user at Acme sees the dataset as a source, and Acme's rows only.
    let reader = login(&app, &user(&pools, Some(("read_only", acme))).await.1).await;
    let (_, catalog) = call(
        &app,
        &reader,
        "GET",
        "/api/v1/analytics/sources",
        Value::Null,
    )
    .await;
    let computers = catalog["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "dataset:computers")
        .unwrap()
        .clone();
    let fields: Vec<&str> = computers["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        [
            "_organization",
            "hostname",
            "ram_gb",
            "last_seen",
            "_synced_at"
        ]
    );
    assert_eq!(computers["timeField"], "last_seen");
    let by_org = json!({ "source": "dataset:computers", "measures": [{ "fn": "sum", "field": "ram_gb" }, { "fn": "count" }],
                         "dimensions": [{ "field": "_organization" }] });
    let (status, answer) = call(
        &app,
        &reader,
        "POST",
        "/api/v1/analytics/query",
        by_org.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["rows"][0][1], 24.0, "{answer}");
    assert_eq!(answer["rows"].as_array().unwrap().len(), 1);
    // The owner sees both.
    let (_, answer) = call(&app, &owner, "POST", "/api/v1/analytics/query", by_org).await;
    assert_eq!(answer["rows"].as_array().unwrap().len(), 2, "{answer}");
    refusals(&app, &owner).await;
}

/// Unknown datasets and columns are refused.
async fn refusals(app: &axum::Router, owner: &str) {
    let (status, _) = call(
        app,
        owner,
        "POST",
        "/api/v1/analytics/query",
        json!({ "source": "dataset:nope", "measures": [{ "fn": "count" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, problem) = call(
        app,
        owner,
        "POST",
        "/api/v1/analytics/query",
        json!({ "source": "dataset:computers", "dimensions": [{ "field": "raw" }] }),
    )
    .await;
    assert_eq!(
        (status, &problem["errors"][0]["pointer"]),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            &json!("/dimensions/0/field")
        )
    );
}

/// A submission's decision endpoint (`approve` or `reject`).
fn decide(sub: &Value, how: &str) -> String {
    format!(
        "/api/v1/dashboard-submissions/{}/{how}",
        sub["id"].as_str().unwrap()
    )
}

/// Names of the stored (not prebuilt) dashboards a caller sees.
async fn stored_names(app: &axum::Router, cookie: &str) -> Vec<String> {
    let (_, all) = call(app, cookie, "GET", "/api/v1/dashboards", Value::Null).await;
    all.as_array()
        .unwrap()
        .iter()
        .filter(|d| d["builtin"] == false)
        .map(|d| d["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn dashboards_are_submitted_approved_and_published_to_an_audience() {
    let Some(db) = isolated("dashboard_publishing").await else {
        return;
    };
    let pools = db.pools.clone();
    let app = app_for(&pools);
    let (acme, team, [approver, author, mate, outsider]) = publishing_people(&app, &pools).await;
    let widget = json!({ "id": "open", "title": "Tickets", "viz": "number", "width": 3,
                         "spec": { "source": "tickets", "measures": [{ "fn": "count" }] } });
    let (status, board) = call(
        &app,
        &author,
        "POST",
        "/api/v1/dashboards",
        json!({ "orgId": acme, "dashboard": { "name": "Ops board", "widgets": [widget] } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{board}");
    let id = board["id"].as_str().unwrap().to_owned();
    let submit = format!("/api/v1/dashboards/{id}/submit");
    let ask = json!({ "teams": [team], "portal": true, "note": "For the ops team and Acme" });
    let (status, first) = call(&app, &author, "POST", &submit, ask.clone()).await;
    assert_eq!(
        (status, &first["status"]),
        (StatusCode::CREATED, &json!("pending")),
        "{first}"
    );
    assert_eq!(
        call(&app, &author, "POST", &submit, ask.clone()).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    first_is_sent_back(&app, &approver, &author, &mate, &first).await;
    // Resubmitted and approved: shared with the team and the portal, as submitted.
    let (_, second) = call(&app, &author, "POST", &submit, ask).await;
    lookups(&app, &author, &approver, &mate, &second, team).await;
    let (_, waiting) = call(
        &app,
        &approver,
        "GET",
        "/api/v1/dashboard-submissions",
        Value::Null,
    )
    .await;
    assert!(
        waiting
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == second["id"]),
        "{waiting}"
    );
    let (status, publication) = call(
        &app,
        &approver,
        "POST",
        &decide(&second, "approve"),
        json!({}),
    )
    .await;
    assert_eq!(
        (status, &publication["number"]),
        (StatusCode::OK, &json!(1)),
        "{publication}"
    );
    assert_eq!(
        call(
            &app,
            &approver,
            "POST",
            &decide(&second, "approve"),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    published(
        &app,
        &pools,
        &id,
        team,
        [&approver, &author, &mate, &outsider],
    )
    .await;

    copies(&app, &outsider, acme).await;
}

/// After approval: the team and its author see it, others don't; it's
/// shared, in the portal, with its history and events.
async fn published(app: &axum::Router, pools: &TestPools, id: &str, team: Uuid, who: [&String; 4]) {
    let [approver, author, mate, outsider] = who;
    let app = app.clone();
    let (approver, author, mate, outsider) = (
        approver.as_str(),
        author.as_str(),
        mate.as_str(),
        outsider.as_str(),
    );
    assert_eq!(stored_names(&app, mate).await, ["Ops board"]);
    assert_eq!(stored_names(&app, author).await, ["Ops board"]);
    assert_eq!(stored_names(&app, outsider).await, Vec::<String>::new());
    let (_, published) = call(
        &app,
        approver,
        "GET",
        &format!("/api/v1/dashboards/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (&published["ownerUserId"], &published["portal"]),
        (&Value::Null, &json!(true)),
        "{published}"
    );
    assert_eq!(published["audienceTeams"], json!([team]));
    let (_, history) = call(
        &app,
        author,
        "GET",
        &format!("/api/v1/dashboards/{id}/submissions"),
        Value::Null,
    )
    .await;
    let statuses: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["approved", "rejected"]);
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events_outbox WHERE subject_id = $1 ORDER BY occurred_at, id",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert_eq!(
        events,
        [
            "dashboard.submitted",
            "dashboard.rejected",
            "dashboard.submitted",
            "dashboard.published"
        ]
    );
}

/// Acme; an approver (root owner); three technicians at Acme (an author, a
/// teammate in the Ops team, an outsider); the team.
async fn publishing_people(app: &axum::Router, pools: &TestPools) -> (Uuid, Uuid, [String; 4]) {
    let (app, pools) = (app.clone(), pools.clone());
    let root = ensure_root(&pools).await;
    let acme = org(&pools, root, huxley_orgs::OrgKind::Client, "Acme").await;
    let approver = login(&app, &user(&pools, None).await.1).await;
    let (_, author_email) = user(&pools, Some(("technician", acme))).await;
    let author = login(&app, &author_email).await;
    let (mate_id, mate_email) = user(&pools, Some(("technician", acme))).await;
    let mate = login(&app, &mate_email).await;
    let outsider = login(&app, &user(&pools, Some(("technician", acme))).await.1).await;
    let team = Uuid::now_v7();
    sqlx::query("INSERT INTO teams (id, name) VALUES ($1, $2)")
        .bind(team)
        .bind(unique("Ops"))
        .execute(&pools.owner)
        .await
        .unwrap();
    sqlx::query("INSERT INTO team_members (team_id, user_id) VALUES ($1, $2)")
        .bind(team)
        .bind(mate_id)
        .execute(&pools.owner)
        .await
        .unwrap();

    (acme, team, [approver, author, mate, outsider])
}

/// Anyone copies what they can see into a personal dashboard.
async fn copies(app: &axum::Router, outsider: &str, acme: Uuid) {
    let (status, copy) = call(
        app,
        outsider,
        "POST",
        "/api/v1/dashboards/service-desk/copy",
        json!({ "orgId": acme }),
    )
    .await;
    assert_eq!(
        (status, &copy["name"]),
        (StatusCode::CREATED, &json!("Copy of Service desk overview")),
        "{copy}"
    );
    assert_eq!(
        stored_names(app, outsider).await,
        ["Copy of Service desk overview"]
    );
}

/// Only someone who manages dashboards there decides, and a rejection says
/// why; it stays personal.
async fn first_is_sent_back(
    app: &axum::Router,
    approver: &str,
    author: &str,
    mate: &str,
    first: &Value,
) {
    assert_eq!(
        call(app, author, "POST", &decide(first, "approve"), json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(app, approver, "POST", &decide(first, "reject"), json!({}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, rejected) = call(
        app,
        approver,
        "POST",
        &decide(first, "reject"),
        json!({ "comment": "Add a title that says it's Acme's" }),
    )
    .await;
    assert_eq!(
        (status, &rejected["status"]),
        (StatusCode::OK, &json!("rejected")),
        "{rejected}"
    );
    assert_eq!(stored_names(app, mate).await, Vec::<String>::new());
}

/// Authors see the audiences to pick from; a submission is its author's
/// and its approvers' to see.
async fn lookups(
    app: &axum::Router,
    author: &str,
    approver: &str,
    mate: &str,
    second: &Value,
    team: Uuid,
) {
    let (_, audiences) = call(
        app,
        author,
        "GET",
        "/api/v1/dashboard-audiences",
        Value::Null,
    )
    .await;
    assert!(
        audiences["teams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == json!(team)),
        "{audiences}"
    );
    let one = format!(
        "/api/v1/dashboard-submissions/{}",
        second["id"].as_str().unwrap()
    );
    assert_eq!(
        call(app, approver, "GET", &one, Value::Null).await.1["snapshot"]["name"],
        "Ops board"
    );
    assert_eq!(
        call(app, mate, "GET", &one, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}
