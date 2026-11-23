//! Workflow and run endpoints over HTTP, against a real PostgreSQL.

use std::time::{Duration, Instant};

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

const PASSWORD: &str = "workflow api passphrase";

fn crypto(pools: &TestPools) -> Crypto {
    Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring())
}

fn app(pools: &TestPools) -> axum::Router {
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    router(AppState::new(
        pools.app.clone(),
        auth,
        crypto(pools),
        config,
    ))
}

async fn user(pools: &TestPools, owner: bool) -> (Uuid, String) {
    let email = format!("{}@example.com", unique("wf").replace(' ', "."));
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let u = huxley_auth::create_user(&mut tx, &ctx, &email, "Workflow User", PASSWORD)
        .await
        .unwrap();
    if owner {
        huxley_authz::grant_root_owner(&mut tx, &ctx, u.id)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    (u.id, email)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
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
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

async fn provider_org(pools: &TestPools) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Provider,
        &unique("Wf"),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    org.id
}

fn document(key: &str, steps: &Value) -> Value {
    json!({
        "apiVersion": "usehuxley.com/v1",
        "kind": "Workflow",
        "metadata": { "key": key, "name": "Greeter" },
        "spec": {
            "inputs": {
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string", "minLength": 1 } }
            },
            "steps": steps,
            "outputs": { "greeting": "{{ steps.greet.output }}" }
        }
    })
}

/// Runs the engine until the run finishes; returns the run.
async fn finish(pools: &TestPools, app: &axum::Router, cookie: &str, run: &str) -> Value {
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto(pools));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.drain("api-test", 50).await.unwrap();
        let (status, body) = call(app, "GET", &format!("/api/v1/runs/{run}"), cookie, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if ["succeeded", "failed", "cancelled"].contains(&body["status"].as_str().unwrap()) {
            return body;
        }
        assert!(Instant::now() < deadline, "run didn't finish: {body}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct Setup {
    pools: TestPools,
    app: axum::Router,
    org: Uuid,
    owner: String,
}

async fn setup() -> Option<Setup> {
    Some(setup_in(pools().await?).await)
}

async fn setup_in(pools: TestPools) -> Setup {
    let org = provider_org(&pools).await;
    let app = app(&pools);
    let owner = login(&app, &user(&pools, true).await.1).await;
    Setup {
        pools,
        app,
        org,
        owner,
    }
}

fn key(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

fn greeter(key: &str) -> Value {
    document(
        key,
        &json!([{ "id": "greet", "type": "transform", "output": "Hi {{ inputs.name }}" }]),
    )
}

impl Setup {
    async fn post(&self, cookie: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        call(&self.app, "POST", uri, cookie, Some(body)).await
    }

    async fn get(&self, cookie: &str, uri: &str) -> (StatusCode, Value) {
        call(&self.app, "GET", uri, cookie, None).await
    }

    async fn create(&self, doc: Value) -> (StatusCode, Value) {
        let body = json!({ "orgId": self.org, "document": doc });
        self.post(&self.owner, "/api/v1/workflows", body).await
    }
}

#[tokio::test]
async fn drafts_save_with_problems_and_publishing_checks_them() {
    let Some(s) = setup().await else { return };
    let key = key("greet");
    let broken = document(
        &key,
        &json!([{ "id": "greet", "type": "transform", "output": "Hi {{ steps.nope.output }}" }]),
    );
    let (status, created) = s.create(broken.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["revision"], 1);
    assert_eq!(created["diagnostics"][0]["path"], "spec.steps[0].output");
    assert_eq!(s.create(broken).await.0, StatusCode::CONFLICT, "same key");

    let publish = format!("/api/v1/workflows/{id}/publish");
    let (status, problem) = s.post(&s.owner, &publish, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["errors"][0]["pointer"], "spec.steps[0].output");

    // Fix it; a stale revision is refused.
    let draft = format!("/api/v1/workflows/{id}/draft");
    let body = json!({ "revision": 1, "document": greeter(&key) });
    let (status, saved) = call(&s.app, "PUT", &draft, &s.owner, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["revision"], 2);
    assert_eq!(saved["diagnostics"], json!([]));
    let (status, _) = call(&s.app, "PUT", &draft, &s.owner, Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, version) = s
        .post(&s.owner, &publish, json!({ "notes": "first" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{version}");
    assert_eq!(version["version"], 1);
    let (_, versions) = s
        .get(&s.owner, &format!("/api/v1/workflows/{id}/versions"))
        .await;
    assert_eq!(versions[0]["notes"], "first");
}

#[tokio::test]
async fn test_runs_published_runs_and_history() {
    let Some(s) = setup().await else { return };
    let (_, created) = s.create(greeter(&key("run"))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let runs = format!("/api/v1/workflows/{id}/runs");
    let ada = json!({ "inputs": { "name": "Ada" } });

    // Nothing published yet: only a test run of the draft works.
    assert_eq!(
        s.post(&s.owner, &runs, ada.clone()).await.0,
        StatusCode::CONFLICT
    );
    let (status, test_run) = s
        .post(
            &s.owner,
            &runs,
            json!({ "inputs": { "name": "Ada" }, "test": true }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{test_run}");
    let run = finish(&s.pools, &s.app, &s.owner, test_run["id"].as_str().unwrap()).await;
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["workflowVersion"], Value::Null);
    assert_eq!(run["outputs"], json!({ "greeting": "Hi Ada" }));
    assert_eq!(run["steps"][0]["stepKey"], "greet");

    // Published: inputs are checked against the schema.
    s.post(
        &s.owner,
        &format!("/api/v1/workflows/{id}/publish"),
        json!({}),
    )
    .await;
    let (status, bad) = s
        .post(&s.owner, &runs, json!({ "inputs": { "name": "" } }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    let (status, started) = s
        .post(&s.owner, &runs, json!({ "inputs": { "name": "Bo" } }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let run_id = started["id"].as_str().unwrap().to_owned();
    let run = finish(&s.pools, &s.app, &s.owner, &run_id).await;
    assert_eq!(run["workflowVersion"], 1);
    assert_eq!(run["outputs"], json!({ "greeting": "Hi Bo" }));

    let (_, listed) = s
        .get(&s.owner, &format!("/api/v1/runs?workflowId={id}"))
        .await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["id"], run_id.as_str(), "newest first");
    let (_, workflows) = s.get(&s.owner, "/api/v1/workflows").await;
    assert!(
        workflows
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == id.as_str() && w["currentVersion"] == 1)
    );

    // Cancelling a finished run does nothing; archived workflows don't run.
    let (status, cancelled) = s
        .post(
            &s.owner,
            &format!("/api/v1/runs/{run_id}/cancel"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["cancelled"], false);
    let archive = format!("/api/v1/workflows/{id}/archive");
    assert_eq!(
        s.post(&s.owner, &archive, json!({})).await.0,
        StatusCode::OK
    );
    assert_eq!(s.post(&s.owner, &runs, ada).await.0, StatusCode::CONFLICT);
}

/// A published greeter workflow and a technician on the org.
async fn with_technician(s: &Setup) -> (String, String) {
    let (tech_id, tech_email) = user(&s.pools, false).await;
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id) \
         SELECT $1, $2, id, $3 FROM roles WHERE key = 'technician'",
    )
    .bind(Uuid::now_v7())
    .bind(tech_id)
    .bind(s.org)
    .execute(&s.pools.owner)
    .await
    .unwrap();
    let (_, created) = s.create(greeter(&key("perm"))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    s.post(
        &s.owner,
        &format!("/api/v1/workflows/{id}/publish"),
        json!({}),
    )
    .await;
    (id, login(&s.app, &tech_email).await)
}

#[tokio::test]
async fn technicians_view_and_run_but_dont_edit() {
    let Some(s) = setup().await else { return };
    let (id, tech) = with_technician(&s).await;
    assert_eq!(
        s.get(&tech, &format!("/api/v1/workflows/{id}")).await.0,
        StatusCode::OK
    );
    let new = json!({ "orgId": s.org, "document": greeter("other") });
    assert_eq!(
        s.post(&tech, "/api/v1/workflows", new).await.0,
        StatusCode::FORBIDDEN
    );
    let draft = json!({ "revision": 1, "document": greeter("x") });
    let uri = format!("/api/v1/workflows/{id}/draft");
    assert_eq!(
        call(&s.app, "PUT", &uri, &tech, Some(draft)).await.0,
        StatusCode::FORBIDDEN
    );
    let publish = format!("/api/v1/workflows/{id}/publish");
    assert_eq!(
        s.post(&tech, &publish, json!({})).await.0,
        StatusCode::FORBIDDEN
    );
    let runs = format!("/api/v1/workflows/{id}/runs");
    let test = json!({ "inputs": { "name": "T" }, "test": true });
    assert_eq!(
        s.post(&tech, &runs, test).await.0,
        StatusCode::FORBIDDEN,
        "test runs need edit rights"
    );
    let (status, run) = s
        .post(&tech, &runs, json!({ "inputs": { "name": "T" } }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
}

#[tokio::test]
async fn strangers_see_nothing_and_validation_is_open() {
    let Some(s) = setup().await else { return };
    let (id, tech) = with_technician(&s).await;
    let runs = format!("/api/v1/workflows/{id}/runs");
    let (_, run) = s
        .post(&tech, &runs, json!({ "inputs": { "name": "T" } }))
        .await;
    let stranger = login(&s.app, &user(&s.pools, false).await.1).await;

    assert_eq!(
        s.get(&stranger, &format!("/api/v1/workflows/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
    let run_uri = format!("/api/v1/runs/{}", run["id"].as_str().unwrap());
    assert_eq!(s.get(&stranger, &run_uri).await.0, StatusCode::NOT_FOUND);
    let (_, listed) = s.get(&stranger, "/api/v1/workflows").await;
    assert!(
        !listed
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == id.as_str())
    );

    let (status, checked) = s
        .post(
            &stranger,
            "/api/v1/workflows/validate",
            json!({ "document": { "apiVersion": "nope" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(checked["diagnostics"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn dead_jobs_are_listed_and_retried_by_admins() {
    let Some(s) = setup().await else { return };
    let job = Uuid::now_v7();
    // A dead advance for a run that doesn't exist: harmless once retried.
    sqlx::query(
        "INSERT INTO jobs (id, kind, run_id, attempts, dead, last_error, run_at) \
         VALUES ($1, 'advance', $2, 10, true, 'database went away', now() + interval '1 hour')",
    )
    .bind(job)
    .bind(Uuid::now_v7())
    .execute(&s.pools.owner)
    .await
    .unwrap();
    let (status, dead) = s.get(&s.owner, "/api/v1/jobs/dead").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        dead.as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == job.to_string() && j["lastError"] == "database went away")
    );
    let (_, tech) = with_technician(&s).await;
    assert_eq!(
        s.get(&tech, "/api/v1/jobs/dead").await.0,
        StatusCode::FORBIDDEN
    );

    let retry = format!("/api/v1/jobs/{job}/retry");
    let (status, body) = s.post(&s.owner, &retry, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["retried"], true);
    let (_, again) = s.post(&s.owner, &retry, json!({})).await;
    assert_eq!(again["retried"], false);
    sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(job)
        .execute(&s.pools.owner)
        .await
        .unwrap();
}

#[tokio::test]
async fn spilled_step_outputs_are_fetched_in_full() {
    // Its own database: an engine in another test process (without this
    // test's store) mustn't run this test's jobs.
    let Some(db) = isolated("api_spill").await else {
        return;
    };
    let s = setup_in(db.pools.clone()).await;
    // An app and an engine that share one store, spilling anything over 1 KB.
    let storage = huxley_storage::Storage::memory();
    let auth = AuthService::new(s.pools.app.clone(), AuthConfig::default(), crypto(&s.pools));
    let app = router(
        AppState::new(
            s.pools.app.clone(),
            auth,
            crypto(&s.pools),
            ApiConfig {
                login_attempts_per_minute: 1000,
                ..ApiConfig::default()
            },
        )
        .with_storage(storage.clone()),
    );
    let engine = huxley_engine::Engine::with_config(
        s.pools.app.clone(),
        crypto(&s.pools),
        &huxley_engine::EngineConfig {
            payload: huxley_engine::PayloadConfig { spill_kb: 1 },
            ..huxley_engine::EngineConfig::default()
        },
    )
    .with_storage(storage);

    let mut doc = document(
        &key("spill"),
        &json!([
            { "id": "big", "type": "transform", "output": "{{ range(2000) | join(',') }}" },
            { "id": "small", "type": "transform", "output": "{{ inputs.name }}" }
        ]),
    );
    doc["spec"]["outputs"] = json!({});
    let (_, created) = s.create(doc).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let (status, started) = call(
        &app,
        "POST",
        &format!("/api/v1/workflows/{id}/runs"),
        &s.owner,
        Some(json!({ "inputs": { "name": "Ada" }, "test": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let run_id = started["id"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(20);
    let run = loop {
        engine.drain("api-test", 50).await.unwrap();
        let (_, run) = call(
            &app,
            "GET",
            &format!("/api/v1/runs/{run_id}"),
            &s.owner,
            None,
        )
        .await;
        if run["status"] == "succeeded" {
            break run;
        }
        assert!(Instant::now() < deadline, "run didn't finish: {run}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let expected: String = (0..2000)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let big = &run["steps"][0];
    assert_eq!(big["stepKey"], "big");
    assert!(
        big["output"]["$spilled"]["bytes"].as_u64().unwrap() > 1024,
        "{big}"
    );
    let payload = |key: &str, field: &str| {
        let uri = format!("/api/v1/runs/{run_id}/steps/{key}/1/{field}");
        let (app, cookie) = (app.clone(), s.owner.clone());
        async move { call(&app, "GET", &uri, &cookie, None).await }
    };
    assert_eq!(
        payload("big", "output").await,
        (StatusCode::OK, json!(expected))
    );
    // Values that weren't spilled come straight from the row.
    assert_eq!(
        payload("small", "output").await,
        (StatusCode::OK, json!("Ada"))
    );
    assert_eq!(payload("nope", "output").await.0, StatusCode::NOT_FOUND);
    assert_eq!(payload("big", "sideways").await.0, StatusCode::BAD_REQUEST);

    // Someone without access to the run can't read it.
    let (_, email) = user(&s.pools, false).await;
    let stranger = login(&app, &email).await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/runs/{run_id}/steps/big/1/output"),
        &stranger,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Reads SSE `run` events from a response body until it ends.
async fn run_events(body: axum::body::Body, until: Duration) -> Vec<Value> {
    use futures_util::StreamExt as _;
    let mut stream = body.into_data_stream();
    let mut buffer = String::new();
    let mut events = Vec::new();
    let deadline = Instant::now() + until;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(Some(chunk)) = tokio::time::timeout(remaining, stream.next()).await else {
            break;
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        while let Some(end) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..end + 2).collect();
            if frame.lines().any(|l| l == "event: run") {
                let data: String = frame
                    .lines()
                    .filter_map(|l| l.strip_prefix("data: "))
                    .collect();
                events.push(serde_json::from_str(&data).unwrap());
            }
        }
    }
    events
}

#[tokio::test]
async fn run_streams_follow_a_run_until_it_finishes() {
    // Its own database: this test's engine runs the jobs.
    let Some(db) = isolated("api_stream").await else {
        return;
    };
    let pools = db.pools.clone();
    let auth = AuthService::new(pools.app.clone(), AuthConfig::default(), crypto(&pools));
    let state = AppState::new(
        pools.app.clone(),
        auth,
        crypto(&pools),
        ApiConfig {
            login_attempts_per_minute: 1000,
            ..ApiConfig::default()
        },
    );
    let stop = tokio_util::sync::CancellationToken::new();
    tokio::spawn(
        state
            .run_changes
            .clone()
            .run(pools.app.clone(), stop.clone()),
    );
    let app = router(state);
    let org = provider_org(&pools).await;
    let owner = login(&app, &user(&pools, true).await.1).await;
    let mut doc = document(
        &key("live"),
        &json!([
            { "id": "first", "type": "transform", "output": 1 },
            { "id": "pause", "type": "wait", "duration": "50ms" },
            { "id": "last", "type": "transform", "output": 2 }
        ]),
    );
    doc["spec"]["outputs"] = json!({ "last": "{{ steps.last.output }}" });
    let (_, created) = call(
        &app,
        "POST",
        "/api/v1/workflows",
        &owner,
        Some(json!({ "orgId": org, "document": doc })),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    let (status, started) = call(
        &app,
        "POST",
        &format!("/api/v1/workflows/{id}/runs"),
        &owner,
        Some(json!({ "inputs": { "name": "Ada" }, "test": true })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let run_id = started["id"].as_str().unwrap().to_owned();
    tokio::time::sleep(Duration::from_millis(300)).await; // let it LISTEN

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/runs/{run_id}/stream"))
                .header("cookie", &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let events = tokio::spawn(run_events(response.into_body(), Duration::from_secs(20)));
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto(&pools));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !events.is_finished() {
        engine.drain("api-test", 50).await.unwrap();
        assert!(Instant::now() < deadline, "the stream didn't end");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let events = events.await.unwrap();
    stop.cancel();

    assert!(events.len() >= 2, "{events:?}");
    assert_eq!(events[0]["id"], run_id.as_str());
    assert_eq!(events[0]["status"], "queued");
    let last = events.last().unwrap();
    assert_eq!(
        last["status"], "succeeded",
        "the stream ends on the finished run"
    );
    assert_eq!(last["outputs"], json!({ "last": 2 }));
    assert_eq!(last["steps"].as_array().unwrap().len(), 3);

    // Someone who can't see the run can't stream it.
    let (_, email) = user(&pools, false).await;
    let stranger = login(&app, &email).await;
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/runs/{run_id}/stream"),
        &stranger,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_as_needs_an_active_service_account_and_the_right_to_assign_it() {
    let Some(s) = setup().await else { return };
    let (_, created) = s.create(greeter(&key("runas"))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["runAs"], Value::Null);
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let bot = huxley_auth::api_tokens::create_service_account(&mut tx, &ctx, &unique("Bot"), "")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let uri = format!("/api/v1/workflows/{id}/run-as");
    let put = |cookie: String, body: Value| {
        let (app, uri) = (s.app.clone(), uri.clone());
        async move { call(&app, "PUT", &uri, &cookie, Some(body)).await }
    };

    let (status, summary) = put(s.owner.clone(), json!({ "serviceAccountId": bot.id })).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["runAs"], json!(bot.id));
    assert_eq!(summary["runAsName"], json!(bot.display_name));

    // People aren't service accounts.
    let (person, _) = user(&s.pools, false).await;
    let (status, problem) = put(s.owner.clone(), json!({ "serviceAccountId": person })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");

    // Publishing rights alone aren't enough to hand out an account.
    let (publisher, email) = user(&s.pools, false).await;
    let role = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO roles (id, key, name, permissions) \
         VALUES ($1, $2, 'Publisher', '{workflow:read,workflow:publish}')",
    )
    .bind(role)
    .bind(format!("publisher_{}", &role.simple().to_string()[20..]))
    .execute(&s.pools.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(publisher)
    .bind(role)
    .bind(s.org)
    .execute(&s.pools.owner)
    .await
    .unwrap();
    let publisher = login(&s.app, &email).await;
    let (status, _) = put(publisher, json!({ "serviceAccountId": bot.id })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, summary) = put(s.owner.clone(), json!({ "serviceAccountId": null })).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["runAs"], Value::Null);
}

#[tokio::test]
async fn approvals_are_for_their_approvers() {
    // Its own database: this test's engine runs the jobs.
    let Some(db) = isolated("api_approvals").await else {
        return;
    };
    let s = setup_in(db.pools.clone()).await;
    let (approver_id, approver_email) = user(&s.pools, false).await;
    let doc = json!({
        "apiVersion": "usehuxley.com/v1",
        "kind": "Workflow",
        "metadata": { "key": "ask", "name": "Ask" },
        "spec": { "steps": [
            { "id": "ask", "type": "approval", "title": "OK?", "approvers": [format!("user:{approver_id}")] }
        ] }
    });
    let (_, created) = s.create(doc).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let (status, run) = s
        .post(
            &s.owner,
            &format!("/api/v1/workflows/{id}/runs"),
            json!({ "test": true }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let engine = huxley_engine::Engine::new(s.pools.app.clone(), crypto(&s.pools));
    let approver = login(&s.app, &approver_email).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    let approval = loop {
        engine.drain("api-test", 20).await.unwrap();
        let (_, mine) = s.get(&approver, "/api/v1/approvals").await;
        if let Some(first) = mine.as_array().and_then(|a| a.first()) {
            break first.clone();
        }
        assert!(Instant::now() < deadline, "no approval appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(approval["title"], "OK?");
    assert_eq!(approval["status"], "pending");
    let uri = format!("/api/v1/approvals/{}", approval["id"].as_str().unwrap());

    // Someone else neither sees it listed nor can open it.
    let (_, stranger_email) = user(&s.pools, false).await;
    let stranger = login(&s.app, &stranger_email).await;
    let (_, theirs) = s.get(&stranger, "/api/v1/approvals").await;
    assert_eq!(theirs, json!([]));
    assert_eq!(s.get(&stranger, &uri).await.0, StatusCode::NOT_FOUND);
    // The owner can read the run, so can see the approval, but can't decide.
    assert_eq!(s.get(&s.owner, &uri).await.0, StatusCode::OK);
    let decision = format!("{uri}/decision");
    let (status, _) = s
        .post(&s.owner, &decision, json!({ "decision": "approve" }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, decided) = s
        .post(
            &approver,
            &decision,
            json!({ "decision": "reject", "comment": "not now" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["status"], "rejected");
    let (status, _) = s
        .post(&approver, &decision, json!({ "decision": "approve" }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}
