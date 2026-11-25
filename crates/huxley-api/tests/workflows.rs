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
        "apiVersion": "usehuxley.io/v1",
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
        "apiVersion": "usehuxley.io/v1",
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

#[tokio::test]
async fn forms_start_their_workflows_for_the_org_they_are_for() {
    let Some(s) = setup().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        s.org,
        huxley_orgs::OrgKind::Client,
        &unique("Client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    let form_key = key("new-user");

    // A definition that isn't an object schema is refused at that field.
    let bad = json!({ "orgId": s.org, "form": {
        "key": form_key, "name": "New user", "schema": { "type": "string" } } });
    let (status, problem) = s.post(&s.owner, "/api/v1/forms", bad).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "schema.type");

    let schema = json!({
        "type": "object",
        "required": ["name", "seats"],
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "seats": { "type": "integer", "minimum": 1 }
        }
    });
    let (status, form) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": form_key, "name": "New user", "schema": schema } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{form}");
    let form_id = form["id"].as_str().unwrap().to_owned();

    // Two workflows listen for it; one wants an input the form doesn't have.
    listen(&s, &form_key).await;
    // A technician fills it in for the client.
    let (tech_id, tech) = technician(&s).await;
    let submit = format!("/api/v1/forms/{form_id}/submissions");
    let (status, problem) = s
        .post(
            &tech,
            &submit,
            json!({ "orgId": client, "answers": { "name": "Ada", "seats": 0 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "answers/seats");

    let (status, submitted) = s
        .post(
            &tech,
            &submit,
            json!({ "orgId": client, "answers": { "name": "Ada", "seats": 2 } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{submitted}");
    assert_eq!(submitted["runs"].as_array().unwrap().len(), 1);
    assert_eq!(submitted["skipped"], json!(["picky"]));
    let run: Uuid = submitted["runs"][0].as_str().unwrap().parse().unwrap();
    let (org, kind, actor, inputs): (Uuid, String, Option<Uuid>, Value) =
        sqlx::query_as("SELECT org_id, trigger_type, actor_id, inputs FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert_eq!(org, client, "runs happen in the org it was for");
    assert_eq!(kind, "form");
    assert_eq!(actor, Some(tech_id), "form runs act as the submitter");
    assert_eq!(inputs, json!({ "name": "Ada", "seats": 2 }));

    // An organization outside the form's tree is refused.
    let elsewhere = provider_org(&s.pools).await;
    let (status, _) = s
        .post(
            &s.owner,
            &submit,
            json!({ "orgId": elsewhere, "answers": { "name": "A", "seats": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Strangers can't see or submit it.
    let (_, stranger_email) = user(&s.pools, false).await;
    let stranger = login(&s.app, &stranger_email).await;
    assert_eq!(
        s.get(&stranger, &format!("/api/v1/forms/{form_id}"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

/// Publishes two workflows with a trigger for `form_key`: one whose inputs
/// fit the form, and "picky", which wants an input it doesn't have.
async fn listen(s: &Setup, form_key: &str) {
    for (wf_key, inputs) in [
        (
            "onboard",
            json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } }),
        ),
        (
            "picky",
            json!({ "type": "object", "required": ["department"], "properties": { "department": { "type": "string" } } }),
        ),
    ] {
        let doc = json!({
            "apiVersion": "usehuxley.io/v1", "kind": "Workflow",
            "metadata": { "key": key(wf_key), "name": wf_key },
            "spec": {
                "inputs": inputs,
                "triggers": [{ "type": "form", "form": form_key }],
                "steps": [{ "id": "hi", "type": "transform", "output": "hi" }]
            }
        });
        let (_, created) = s.create(doc).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let (status, _) = s
            .post(
                &s.owner,
                &format!("/api/v1/workflows/{id}/publish"),
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
}

/// A technician at the setup's organization, signed in.
async fn technician(s: &Setup) -> (Uuid, String) {
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
    (tech_id, login(&s.app, &tech_email).await)
}

#[tokio::test]
async fn scripts_keep_every_version() {
    let Some(s) = setup().await else { return };
    let script_key = key("new-user");
    let new = |parameters: Value| {
        json!({ "orgId": s.org, "script": {
            "key": script_key, "name": "New user", "language": "pwsh",
            "source": "param($Upn)\nWrite-Output \"Creating $Upn\"",
            "parameters": parameters } })
    };
    let (status, problem) = s
        .post(
            &s.owner,
            "/api/v1/scripts",
            new(json!({ "type": "string" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "parameters");

    let params = json!({ "type": "object", "required": ["upn"], "properties": { "upn": { "type": "string" } } });
    let (status, script) = s
        .post(&s.owner, "/api/v1/scripts", new(params.clone()))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{script}");
    assert_eq!(script["currentVersion"], 1);
    assert_eq!(script["language"], "pwsh");
    assert_eq!(
        s.post(&s.owner, "/api/v1/scripts", new(params.clone()))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let id = script["id"].as_str().unwrap().to_owned();
    let versions = format!("/api/v1/scripts/{id}/versions");

    // The next version, based on the current one.
    let next = json!({ "source": "param($Upn)\nWrite-Output \"Hello $Upn\"", "parameters": params,
                       "notes": "friendlier", "baseVersion": 1 });
    let (status, saved) = s.post(&s.owner, &versions, next.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["currentVersion"], 2);
    assert!(saved["source"].as_str().unwrap().contains("Hello"));
    // An edit based on version 1 again is stale.
    assert_eq!(
        s.post(&s.owner, &versions, next).await.0,
        StatusCode::CONFLICT
    );

    let (_, history) = s.get(&s.owner, &versions).await;
    assert_eq!(history[0]["version"], 2);
    assert_eq!(history[0]["notes"], "friendlier");
    let (_, first) = s.get(&s.owner, &format!("{versions}/1")).await;
    assert!(first["source"].as_str().unwrap().contains("Creating"));

    // Technicians read the library but can't change it.
    let (_, tech) = technician(&s).await;
    assert_eq!(
        s.get(&tech, &format!("/api/v1/scripts/{id}")).await.0,
        StatusCode::OK
    );
    let edit = json!({ "source": "Write-Output 1" });
    assert_eq!(
        s.post(&tech, &versions, edit).await.0,
        StatusCode::FORBIDDEN
    );
    let (_, stranger_email) = user(&s.pools, false).await;
    let stranger = login(&s.app, &stranger_email).await;
    assert_eq!(
        s.get(&stranger, &format!("/api/v1/scripts/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn runners_enrol_once_and_only_admins_manage_them() {
    // Its own database: the runner CA is one per instance.
    let Some(db) = isolated("api_runners").await else {
        return;
    };
    let s = setup_in(db.pools.clone()).await;
    let (_, tech) = technician(&s).await;
    assert_eq!(
        s.get(&tech, "/api/v1/runners").await.0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = s
        .post(
            &tech,
            "/api/v1/runners/enrollments",
            json!({ "name": "nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, problem) = s
        .post(
            &s.owner,
            "/api/v1/runners/enrollments",
            json!({ "name": "Bad Name" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let (status, token) = s
        .post(
            &s.owner,
            "/api/v1/runners/enrollments",
            json!({ "name": "runner-a" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let token = token["token"].as_str().unwrap().to_owned();

    // The runner enrols without a session; the token works once.
    let (_, csr) = huxley_runners::ca::runner_key_and_csr().unwrap();
    let body = json!({ "token": token, "csrPem": csr });
    let (status, enrolled) = call(
        &s.app,
        "POST",
        "/api/v1/runners/enroll",
        "",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enrolled}");
    assert_eq!(enrolled["name"], "runner-a");
    assert!(
        enrolled["certificatePem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );
    assert!(
        enrolled["caPem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );
    let (status, _) = call(&s.app, "POST", "/api/v1/runners/enroll", "", Some(body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, external) = s
        .post(
            &s.owner,
            "/api/v1/runners/external",
            json!({ "name": "byo-runner" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{external}");
    assert_eq!(external["source"], "external");

    let id = enrolled["runnerId"].as_str().unwrap();
    let (status, revoked) = s
        .post(&s.owner, &format!("/api/v1/runners/{id}/revoke"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(revoked["revokedAt"].is_string());
    let (_, list) = s.get(&s.owner, "/api/v1/runners").await;
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn script_steps_show_where_they_ran_and_their_logs() {
    // Its own database: it runs engine jobs.
    let Some(db) = isolated("api_script_logs").await else {
        return;
    };
    let s = setup_in(db.pools.clone()).await;
    let script = json!({ "orgId": s.org, "script": {
        "key": "hello", "name": "Hello", "language": "python", "source": "result = 1" } });
    let (status, body) = s.post(&s.owner, "/api/v1/scripts", script).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let doc = document(
        &key("script"),
        &json!([{ "id": "greet", "type": "script", "script": "hello" }]),
    );
    let (_, created) = s.create(doc).await;
    let id = created["id"].as_str().unwrap();
    let (status, run) = s
        .post(
            &s.owner,
            &format!("/api/v1/workflows/{id}/runs"),
            json!({ "test": true, "inputs": { "name": "Ada" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let run_id: Uuid = run["id"].as_str().unwrap().parse().unwrap();
    let uri = format!("/api/v1/runs/{run_id}/steps/greet/1/script");

    let engine = huxley_engine::Engine::new(s.pools.app.clone(), crypto(&s.pools));
    let runner = Uuid::now_v7();
    sqlx::query("INSERT INTO runners (id, name, source) VALUES ($1, 'runner-x', 'huxley')")
        .bind(runner)
        .execute(&s.pools.owner)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let task = loop {
        engine.drain("api-test", 50).await.unwrap();
        if let Some(task) = engine
            .claim_script(runner, &[huxley_scripts::Language::Python])
            .await
            .unwrap()
        {
            break task;
        }
        assert!(Instant::now() < deadline, "the script wasn't queued");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let (status, running) = s.get(&s.owner, &uri).await;
    assert_eq!(status, StatusCode::OK, "{running}");
    assert_eq!(running["status"], "running");
    assert_eq!(running["runnerName"], "runner-x");

    let report = huxley_engine::script::Report {
        succeeded: true,
        output: Some(json!(1)),
        error: None,
        exit_code: Some(0),
        logs: "hello from the script\n".into(),
    };
    engine
        .report_script(runner, task.id, &report)
        .await
        .unwrap();
    let done = finish(&s.pools, &s.app, &s.owner, &run_id.to_string()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    let (_, view) = s.get(&s.owner, &uri).await;
    assert_eq!(view["status"], "succeeded");
    assert_eq!(view["exitCode"], 0);
    assert_eq!(view["logs"], "hello from the script\n");
    assert_eq!(view["scriptVersion"], 1);
    // Steps without a script task: not found.
    let (status, _) = s
        .get(
            &s.owner,
            &format!("/api/v1/runs/{run_id}/steps/nope/1/script"),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A vendor API listing users at `GET /users`, plus a `POST /users`.
async fn vendor() -> u16 {
    let app = axum::Router::new().route(
        "/users",
        axum::routing::get(|| async {
            axum::Json(json!({ "value": [
                { "id": "u1", "displayName": "Ada Lovelace" },
                { "id": "u2", "displayName": "Grace Hopper" } ] }))
        })
        .post(|| async { axum::Json(json!({})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// A form whose "user" field is shown for existing users, with choices
/// from the vendor, and whose "reason" field is shown for new ones.
fn access_form(org: Uuid, integration: &str) -> Value {
    json!({ "orgId": org, "form": {
        "key": key("access"), "name": "Access request",
        "schema": { "type": "object", "required": ["kind", "user", "reason"], "properties": {
            "kind": { "type": "string", "enum": ["existing", "new"] },
            "user": { "type": "string", "title": "Who" },
            "reason": { "type": "string" } } },
        "ui": {
            "user": { "show_if": { "field": "kind", "equals": "existing" },
                      "options_from": { "action": format!("{integration}/listUsers"), "items": "value",
                                        "value": "id", "label": "displayName" } },
            "reason": { "show_if": { "field": "kind", "equals": "new" } } } } })
}

/// An app whose engine may reach a local vendor API, and an integration
/// (with a connection) for that API: returns the app and its key.
async fn lookup_app(s: &Setup) -> (axum::Router, String) {
    // An app whose engine may reach the local vendor API.
    let auth = AuthService::new(s.pools.app.clone(), AuthConfig::default(), crypto(&s.pools));
    let config = ApiConfig {
        login_attempts_per_minute: 1000,
        ..ApiConfig::default()
    };
    let mut state = AppState::new(s.pools.app.clone(), auth, crypto(&s.pools), config);
    state.engine = huxley_engine::Engine::with_config(
        s.pools.app.clone(),
        crypto(&s.pools),
        &huxley_engine::EngineConfig {
            egress: huxley_engine::EgressConfig {
                private_hosts: vec!["127.0.0.1".into()],
                ..huxley_engine::EgressConfig::default()
            },
            ..huxley_engine::EngineConfig::default()
        },
    );
    let app = router(state);
    let port = vendor().await;
    let integration = key("dir");
    let spec = format!(
        "openapi: 3.0.3\ninfo: {{ title: Dir, version: '1' }}\nservers: [ {{ url: 'http://127.0.0.1:{port}' }} ]\n\
         paths:\n  /users:\n    get: {{ operationId: listUsers, responses: {{ '200': {{ description: ok }} }} }}\n\
         \x20   post: {{ operationId: createUser, responses: {{ '200': {{ description: ok }} }} }}\n"
    );
    let ctx = RequestContext::system(Via::System);
    // The org's first data key audits on its own: make it first.
    crypto(&s.pools)
        .encrypt(
            s.org,
            huxley_crypto::Binding {
                purpose: "test",
                record: s.org,
            },
            b"x",
        )
        .await
        .unwrap();
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let created =
        huxley_integrations::store::create(&mut tx, &ctx, s.org, Some(&integration), &spec)
            .await
            .unwrap();
    let input = huxley_integrations::connections::ConnectionInput {
        name: "Main".into(),
        key: Some("main".into()),
        ..huxley_integrations::connections::ConnectionInput::default()
    };
    huxley_integrations::connections::create(
        &mut tx,
        &crypto(&s.pools),
        &ctx,
        s.org,
        created.summary.id,
        &input,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    (app, integration)
}

#[tokio::test]
async fn forms_show_fields_conditionally_and_look_up_choices() {
    let Some(s) = setup().await else { return };
    let (app, integration) = lookup_app(&s).await;
    let form = access_form(s.org, &integration);
    let (status, created) = call(&app, "POST", "/api/v1/forms", &s.owner, Some(form)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();

    // The looked-up choices.
    let (status, choices) = call(
        &app,
        "GET",
        &format!("/api/v1/forms/{id}/fields/user/options"),
        &s.owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{choices}");
    assert_eq!(
        choices,
        json!([{ "value": "u1", "label": "Ada Lovelace" }, { "value": "u2", "label": "Grace Hopper" }])
    );
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/forms/{id}/fields/reason/options"),
        &s.owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no lookup on that field");

    // "existing": user is asked (and checked); reason is hidden, so not required and dropped.
    let submit = format!("/api/v1/forms/{id}/submissions");
    let (status, problem) = call(
        &app,
        "POST",
        &submit,
        &s.owner,
        Some(json!({ "answers": { "kind": "existing", "user": "u9" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "answers/user");
    assert_eq!(problem["errors"][0]["message"], "not one of the choices");
    let (status, submitted) = call(
        &app,
        "POST",
        &submit,
        &s.owner,
        Some(json!({ "answers": { "kind": "existing", "user": "u2", "reason": "ignored" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{submitted}");
    let data: Value = sqlx::query_scalar("SELECT data FROM form_submissions WHERE id = $1")
        .bind(
            submitted["submissionId"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(data, json!({ "kind": "existing", "user": "u2" }));

    // "new": reason is required, user isn't.
    let (status, problem) = call(
        &app,
        "POST",
        &submit,
        &s.owner,
        Some(json!({ "answers": { "kind": "new" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert!(problem.to_string().contains("reason"), "{problem}");

    // Lookups are read-only: a POST action can't be used.
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("UPDATE forms SET ui = jsonb_set(ui, '{user,options_from,action}', to_jsonb($2::text)) WHERE id = $1")
        .bind(id.parse::<Uuid>().unwrap())
        .bind(format!("{integration}/createUser"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, problem) = call(
        &app,
        "GET",
        &format!("/api/v1/forms/{id}/fields/user/options"),
        &s.owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{problem}");
    assert!(
        problem["detail"].as_str().unwrap().contains("read-only"),
        "{problem}"
    );
}

/// A published "Sign up" form (and its workflows): its id, key, and body.
async fn signup_form(s: &Setup) -> (String, String, Value) {
    let form_key = key("signup");
    let (status, form) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": form_key, "name": "Sign up",
            "schema": { "type": "object", "required": ["name"],
                        "properties": { "name": { "type": "string", "minLength": 1 } } } } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{form}");
    listen(s, &form_key).await;
    (form["id"].as_str().unwrap().to_owned(), form_key, form)
}

#[tokio::test]
async fn public_links_take_answers_without_signing_in() {
    let Some(s) = setup().await else { return };
    let (id, _, _) = signup_form(&s).await;

    // Only form managers make links.
    let (_, tech) = technician(&s).await;
    let links = format!("/api/v1/forms/{id}/links");
    assert_eq!(
        s.post(&tech, &links, json!({ "name": "x" })).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, created) = s.post(&s.owner, &links, json!({ "name": "Website" })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("hxf_"));
    let link_id = created["link"]["id"].as_str().unwrap().to_owned();
    let (_, listed) = s.get(&s.owner, &links).await;
    assert_eq!(listed[0]["name"], "Website");
    assert!(listed[0].get("token").is_none(), "the token is shown once");

    // Anyone with the link: no session, no CSRF header.
    let public = format!("/public/forms/{token}");
    let (status, shown) = call(&s.app, "GET", &public, "", None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["name"], "Sign up");
    let (status, problem) = call(
        &s.app,
        "POST",
        &public,
        "",
        Some(json!({ "answers": { "name": "" } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    let (status, received) = call(
        &s.app,
        "POST",
        &public,
        "",
        Some(json!({ "answers": { "name": "Ada" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{received}");
    assert_eq!(
        received,
        json!({ "received": true }),
        "nothing about the runs"
    );
    let (submitted_by, link, runs): (Option<Uuid>, Option<Uuid>, Vec<Uuid>) = sqlx::query_as(
        "SELECT submitted_by, link_id, run_ids FROM form_submissions WHERE form_id = $1",
    )
    .bind(id.parse::<Uuid>().unwrap())
    .fetch_one(&s.pools.owner)
    .await
    .unwrap();
    assert_eq!(submitted_by, None);
    assert_eq!(link.map(|l| l.to_string()), Some(link_id.clone()));
    assert_eq!(runs.len(), 1);
    let actor: Option<Uuid> = sqlx::query_scalar("SELECT actor_id FROM runs WHERE id = $1")
        .bind(runs[0])
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(actor, None, "no run-as account set: the run acts as no one");

    // Revoked, or unknown: gone.
    let (status, _) = s
        .post(&s.owner, &format!("{links}/{link_id}/revoke"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&s.app, "GET", &public, "", None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = call(
        &s.app,
        "POST",
        &public,
        "",
        Some(json!({ "answers": { "name": "Ada" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        call(&s.app, "GET", "/public/forms/hxf_nope", "", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn public_forms_never_look_choices_up() {
    let Some(s) = setup().await else { return };
    let (id, form_key, form) = signup_form(&s).await;
    let links = format!("/api/v1/forms/{id}/links");
    let (status, created) = s.post(&s.owner, &links, json!({ "name": "Website" })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let link_id = created["link"]["id"].as_str().unwrap().to_owned();

    // Looked-up choices can't be added while the form is public.
    let ui = json!({ "name": { "options_from": { "action": "graph/users", "value": "id" } } });
    let body = json!({ "key": form_key, "name": "Sign up", "schema": form["schema"], "ui": ui });
    let update = format!("/api/v1/forms/{id}");
    let (status, problem) = call(&s.app, "PUT", &update, &s.owner, Some(body.clone())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert!(problem.to_string().contains("public links"), "{problem}");

    // Once the link is off it can, and then it can't get a link.
    s.post(&s.owner, &format!("{links}/{link_id}/revoke"), json!({}))
        .await;
    let (status, saved) = call(&s.app, "PUT", &update, &s.owner, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, problem) = s.post(&s.owner, &links, json!({ "name": "Again" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
}

#[tokio::test]
async fn secret_form_answers_are_kept_out_of_the_record() {
    let Some(s) = setup().await else { return };
    let form_key = key("reset");
    let (status, form) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": form_key, "name": "Reset",
            "schema": { "type": "object", "required": ["name", "pin"], "properties": {
                "name": { "type": "string" },
                "pin": { "type": "string", "writeOnly": true, "minLength": 4 } } } } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{form}");
    // The workflow doesn't mark it secret: the form's marking is enough.
    listen(&s, &form_key).await;
    let id = form["id"].as_str().unwrap();
    let (status, submitted) = s
        .post(
            &s.owner,
            &format!("/api/v1/forms/{id}/submissions"),
            json!({ "answers": { "name": "Ada", "pin": "PIN-zqxwk" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{submitted}");
    let run: Uuid = submitted["runs"][0].as_str().unwrap().parse().unwrap();
    let (data,): (Value,) = sqlx::query_as("SELECT data FROM form_submissions WHERE form_id = $1")
        .bind(id.parse::<Uuid>().unwrap())
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(data, json!({ "name": "Ada", "pin": "«redacted»" }));
    let (inputs, sealed): (Value, Option<Vec<u8>>) =
        sqlx::query_as("SELECT inputs, inputs_sealed FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&s.pools.owner)
            .await
            .unwrap();
    assert_eq!(inputs["pin"], "«redacted»");
    assert!(sealed.is_some_and(|b| !String::from_utf8_lossy(&b).contains("zqxwk")));
    let (_, shown) = s.get(&s.owner, &format!("/api/v1/runs/{run}")).await;
    assert!(!shown.to_string().contains("zqxwk"), "{shown}");
}

#[tokio::test]
async fn templates_become_drafts_with_their_bindings_resolved() {
    let Some(s) = setup().await else { return };
    let (status, templates) = s.get(&s.owner, "/api/v1/templates").await;
    assert_eq!(status, StatusCode::OK);
    let request = templates
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "request-and-notify")
        .unwrap();
    assert_eq!(request["hasForm"], true);

    // A bad binding is pointed out, and nothing is created.
    let key = key("req");
    let body = |recipients: &str| {
        json!({ "orgId": s.org, "key": key, "name": "Laptop requests",
                "bindings": { "recipients": recipients } })
    };
    let (status, problem) = s
        .post(
            &s.owner,
            "/api/v1/templates/request-and-notify/use",
            body("everyone"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["errors"][0]["pointer"], "bindings.recipients");

    let (status, used) = s
        .post(
            &s.owner,
            "/api/v1/templates/request-and-notify/use",
            body("role:admin, role:technician"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{used}");
    let workflow_id = used["workflowId"].as_str().unwrap();
    let (_, workflow) = s
        .get(&s.owner, &format!("/api/v1/workflows/{workflow_id}"))
        .await;
    assert_eq!(workflow["name"], "Laptop requests");
    assert_eq!(
        workflow["currentVersion"],
        Value::Null,
        "a draft, not published"
    );
    assert_eq!(workflow["diagnostics"], json!([]), "ready to publish");
    assert_eq!(
        workflow["draft"]["spec"]["triggers"][0]["form"], key,
        "its own form"
    );
    assert_eq!(
        workflow["draft"]["spec"]["steps"][0]["with"]["to"],
        json!(["role:admin", "role:technician"])
    );
    let form_id = used["formId"].as_str().unwrap();
    let (_, form) = s.get(&s.owner, &format!("/api/v1/forms/{form_id}")).await;
    assert_eq!(form["key"], key);

    // Used again with the same key: the form's key is taken, nothing new.
    let (status, _) = s
        .post(
            &s.owner,
            "/api/v1/templates/request-and-notify/use",
            body("role:admin"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let workflows: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows WHERE key = $1")
        .bind(&key)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    assert_eq!(workflows, 1);

    // Technicians can't create workflows.
    let (_, tech) = technician(&s).await;
    let (status, _) = s
        .post(
            &tech,
            "/api/v1/templates/webhook-approval/use",
            json!({ "orgId": s.org, "key": "x", "name": "X" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A "Leaver" form and the workflow it starts: the workflow's id.
async fn leaver_workflow(s: &Setup) -> String {
    // A form and the workflow it starts.
    let form_key = key("leaver");
    let (status, _) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": form_key, "name": "Leaver",
            "schema": { "type": "object", "properties": { "who": { "type": "string" } } } } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let doc = json!({
        "apiVersion": "usehuxley.io/v1", "kind": "Workflow",
        "metadata": { "key": key("offboard"), "name": "Offboard" },
        "spec": {
            "triggers": [{ "type": "form", "form": form_key }],
            "steps": [{ "id": "note", "type": "transform", "output": "{{ inputs.who }}" }]
        }
    });
    let (_, workflow) = s.create(doc).await;
    workflow["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn workflows_saved_as_templates_are_offered_down_the_tree() {
    let Some(s) = setup().await else { return };
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    let client = huxley_orgs::create(
        &mut tx,
        &ctx,
        s.org,
        huxley_orgs::OrgKind::Client,
        &unique("Client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();

    let workflow_id = leaver_workflow(&s).await;
    let save = format!("/api/v1/workflows/{workflow_id}/template");
    let (_, tech) = technician(&s).await;
    assert_eq!(
        s.post(&tech, &save, json!({ "name": "X" })).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, saved) = s
        .post(
            &s.owner,
            &save,
            json!({ "name": "Offboarding", "category": "People" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["hasForm"], true);
    let template_id = saved["key"].as_str().unwrap().to_owned();

    // Offered at the client below, next to the built-ins.
    let (_, listed) = s
        .get(&s.owner, &format!("/api/v1/templates?orgId={client}"))
        .await;
    let mine = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == template_id.as_str())
        .unwrap();
    assert_eq!(mine["category"], "People");
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["key"] == "request-and-notify")
    );

    // Used at the client: its own form, and the trigger points at it.
    let new_key = key("off-client");
    let (status, used) = s
        .post(
            &s.owner,
            &format!("/api/v1/templates/{template_id}/use"),
            json!({ "orgId": client, "key": new_key, "name": "Client offboarding" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{used}");
    let new_workflow = used["workflowId"].as_str().unwrap();
    let (_, created) = s
        .get(&s.owner, &format!("/api/v1/workflows/{new_workflow}"))
        .await;
    assert_eq!(created["orgId"], client.to_string());
    assert_eq!(created["draft"]["spec"]["triggers"][0]["form"], new_key);
    let (_, form) = s
        .get(
            &s.owner,
            &format!("/api/v1/forms/{}", used["formId"].as_str().unwrap()),
        )
        .await;
    assert_eq!(form["name"], "Leaver");
    assert_eq!(form["orgId"], client.to_string());

    // Not offered to an organization outside the tree.
    let elsewhere = provider_org(&s.pools).await;
    let (status, _) = s
        .post(
            &s.owner,
            &format!("/api/v1/templates/{template_id}/use"),
            json!({ "orgId": elsewhere, "key": key("x"), "name": "X" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleting: technicians can't; owners can, and it's gone.
    let delete = format!("/api/v1/templates/{template_id}");
    assert_eq!(
        call(&s.app, "DELETE", &delete, &tech, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&s.app, "DELETE", &delete, &s.owner, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, listed) = s
        .get(&s.owner, &format!("/api/v1/templates?orgId={client}"))
        .await;
    assert!(
        !listed
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["key"] == template_id.as_str())
    );
}

#[tokio::test]
async fn expressions_preview_against_the_latest_run() {
    let Some(s) = setup().await else { return };
    let (_, created) = s.create(greeter(&key("preview"))).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let preview = format!("/api/v1/workflows/{id}/preview");

    // No run yet: nothing to preview with.
    let (status, problem) = s
        .post(
            &s.owner,
            &preview,
            json!({ "expression": "{{ inputs.name }}" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert_eq!(
        problem["type"].as_str().unwrap().rsplit('/').next(),
        Some("no-run")
    );

    let (_, run) = s
        .post(
            &s.owner,
            &format!("/api/v1/workflows/{id}/runs"),
            json!({ "inputs": { "name": "Ada" }, "test": true }),
        )
        .await;
    finish(&s.pools, &s.app, &s.owner, run["id"].as_str().unwrap()).await;

    let ask = |expression: Value| json!({ "expression": expression });
    let (status, shown) = s
        .post(
            &s.owner,
            &preview,
            ask(json!(
                "{{ steps.greet.output | upper }} from {{ run.workflow.name }}"
            )),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["value"], "HI ADA from Greeter");
    assert_eq!(shown["runId"], run["id"]);
    // Objects keep their shape.
    let (_, shown) = s
        .post(
            &s.owner,
            &preview,
            ask(json!({ "who": "{{ inputs.name }}" })),
        )
        .await;
    assert_eq!(shown["value"], json!({ "who": "Ada" }));
    // Mistakes are reported, not raised; secrets aren't available.
    let (_, shown) = s.post(&s.owner, &preview, ask(json!("{{ 1 + }}"))).await;
    assert!(shown["error"].is_string(), "{shown}");
    let (_, shown) = s
        .post(&s.owner, &preview, ask(json!("{{ secrets.anything }}")))
        .await;
    assert_ne!(shown["value"], json!("«redacted»"));
    assert!(
        shown["value"].is_null() || shown["value"] == json!(""),
        "{shown}"
    );

    // Technicians can't edit workflows, so can't preview.
    let (_, tech) = technician(&s).await;
    let (status, _) = s.post(&tech, &preview, ask(json!("{{ 1 }}"))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_runs_can_start_at_a_step_with_earlier_results_pinned() {
    let Some(s) = setup().await else { return };
    let doc = json!({
        "apiVersion": "usehuxley.io/v1", "kind": "Workflow",
        "metadata": { "key": key("pin"), "name": "Pinned" },
        "spec": {
            "inputs": { "type": "object", "properties": {
                "name": { "type": "string" }, "extra": { "type": "string" } } },
            "steps": [
                { "id": "first", "type": "transform", "output": "first: {{ inputs.name }}" },
                { "id": "loop", "type": "foreach", "items": "{{ [1, 2] }}", "steps": [
                    { "id": "each", "type": "transform", "output": "{{ item }}" } ] },
                { "id": "second", "type": "transform",
                  "output": "{{ steps.first.output }} / {{ inputs.name }} {{ inputs.extra | default('') }}" }
            ],
            "outputs": { "result": "{{ steps.second.output }}" }
        }
    });
    let (_, created) = s.create(doc).await;
    let id = created["id"].as_str().unwrap().to_owned();
    let runs = format!("/api/v1/workflows/{id}/runs");
    let (_, source) = s
        .post(
            &s.owner,
            &runs,
            json!({ "inputs": { "name": "Ada" }, "test": true }),
        )
        .await;
    let source_id = source["id"].as_str().unwrap().to_owned();
    finish(&s.pools, &s.app, &s.owner, &source_id).await;

    // From `second`, with new inputs: `first` (and the loop) keep the
    // source's results; `second` runs again with the new inputs.
    let (status, run) = s
        .post(
            &s.owner,
            &runs,
            json!({ "inputs": { "name": "Bob", "extra": "!" }, "test": true,
                                        "from": { "runId": source_id, "stepId": "second" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{run}");
    let done = finish(&s.pools, &s.app, &s.owner, run["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["outputs"]["result"], "first: Ada / Bob !");
    let keys: Vec<String> = done["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stepKey"].as_str().unwrap().to_owned())
        .collect();
    for pinned in ["first", "loop", "each[0]", "each[1]"] {
        assert!(keys.iter().any(|k| k == pinned), "{pinned} in {keys:?}");
    }

    // Without inputs, the source's are used.
    let (_, run) = s
        .post(
            &s.owner,
            &runs,
            json!({ "test": true, "from": { "runId": source_id, "stepId": "second" } }),
        )
        .await;
    let done = finish(&s.pools, &s.app, &s.owner, run["id"].as_str().unwrap()).await;
    assert_eq!(done["outputs"]["result"], "first: Ada / Ada ");

    // Only top-level steps, only test runs, only this workflow's runs.
    for (body, pointer) in [
        (
            json!({ "test": true, "from": { "runId": source_id, "stepId": "each" } }),
            "from.stepId",
        ),
        (
            json!({ "from": { "runId": source_id, "stepId": "second" } }),
            "from",
        ),
        (
            json!({ "test": true, "from": { "runId": Uuid::now_v7(), "stepId": "second" } }),
            "from.runId",
        ),
    ] {
        let (status, problem) = s.post(&s.owner, &runs, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert_eq!(problem["errors"][0]["pointer"], pointer, "{problem}");
    }
}

#[tokio::test]
async fn test_cases_run_unsaved_documents_with_mocked_steps() {
    let Some(db) = isolated("api_test_cases").await else {
        return;
    };
    let s = setup_in(db.pools.clone()).await;
    let key = key("case");
    // A saved draft, which the test case must leave alone.
    let (status, saved) = s
        .post(
            &s.owner,
            "/api/v1/workflows",
            json!({ "orgId": s.org, "document": greeter(&key) }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    // The document under test calls an API nobody can reach: its step is
    // mocked, and the mocked output flows on.
    let document = document(
        &key,
        &json!([
            { "id": "lookup", "type": "http.request",
              "with": { "method": "GET", "url": "https://unreachable.invalid/users" } },
            { "id": "greet", "type": "transform",
              "output": "Hi {{ inputs.name }} ({{ steps.lookup.output.body.count }} users)" }
        ]),
    );
    let body = json!({
        "orgId": s.org, "document": document, "inputs": { "name": "Ada" },
        "mocks": { "lookup": { "status": 200, "body": { "count": 3 } } }
    });
    let (status, started) = s.post(&s.owner, "/api/v1/workflows/test-runs", body).await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let run = finish(&s.pools, &s.app, &s.owner, started["id"].as_str().unwrap()).await;
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["outputs"]["greeting"], "Hi Ada (3 users)");
    assert_eq!(run["workflowId"], saved["id"]);
    assert_eq!(run["triggerType"], "test");
    // The draft wasn't touched.
    let (_, detail) = s
        .get(
            &s.owner,
            &format!("/api/v1/workflows/{}", saved["id"].as_str().unwrap()),
        )
        .await;
    assert_eq!(detail["revision"], saved["revision"]);

    // Mocks must name steps; documents must be valid; inputs must fit.
    for (mocks, inputs) in [
        (json!({ "nope": 1 }), json!({ "name": "Ada" })),
        (json!({ "lookup[x]": 1 }), json!({ "name": "Ada" })),
        (json!({}), json!({})),
    ] {
        let body =
            json!({ "orgId": s.org, "document": document, "inputs": inputs, "mocks": mocks });
        let (status, problem) = s.post(&s.owner, "/api/v1/workflows/test-runs", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    }
}

/// Clients Acme (with Ada, and a ticket she raised) and Globex (with a
/// ticket), with CRM and the service desk on: (Acme, Ada's email, Acme's
/// ticket, Globex's ticket).
async fn action_records(s: &Setup) -> (Uuid, String, Value, Value) {
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&s.pools.app, &OrgScope::System).await.unwrap();
    for module in ["crm", "servicedesk"] {
        huxley_modules::set_enabled(&mut tx, &ctx, module, true, false)
            .await
            .unwrap();
    }
    let (client, other) = (
        huxley_orgs::create(
            &mut tx,
            &ctx,
            s.org,
            huxley_orgs::OrgKind::Client,
            &unique("Acme"),
        )
        .await
        .unwrap(),
        huxley_orgs::create(
            &mut tx,
            &ctx,
            s.org,
            huxley_orgs::OrgKind::Client,
            &unique("Globex"),
        )
        .await
        .unwrap(),
    );
    tx.commit().await.unwrap();
    let email = format!("ada.{}@acme.example", Uuid::now_v7().simple());
    let (_, contact) = s
        .post(
            &s.owner,
            &format!("/api/v1/orgs/{}/contacts", client.id),
            json!({ "firstName": "Ada", "email": email }),
        )
        .await;
    let (status, ticket) = s
        .post(
            &s.owner,
            "/api/v1/tickets",
            json!({ "orgId": client.id, "title": "MFA lost", "requesterId": contact["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ticket}");
    let (_, elsewhere) = s
        .post(
            &s.owner,
            "/api/v1/tickets",
            json!({ "orgId": other.id, "title": "Theirs" }),
        )
        .await;

    (client.id, email, ticket, elsewhere)
}

#[tokio::test]
async fn actions_run_with_the_record_they_were_run_on() {
    let Some(s) = setup().await else { return };
    let (client_id, email, ticket, elsewhere) = action_records(&s).await;
    let form_key = key("reset-mfa");
    let schema = json!({ "type": "object", "properties": { "name": { "type": "string" }, "reason": { "type": "string" } } });
    let (status, problem) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": key("odd"), "name": "Odd", "schema": schema, "actionFor": "printer" } }),
        )
        .await;
    assert_eq!(
        (status, problem["errors"][0]["pointer"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("actionFor"))
    );
    let (status, form) = s
        .post(
            &s.owner,
            "/api/v1/forms",
            json!({ "orgId": s.org, "form": {
            "key": form_key, "name": "Reset MFA", "schema": schema, "actionFor": "ticket" } }),
        )
        .await;
    assert_eq!(
        (status, form["actionFor"].as_str()),
        (StatusCode::CREATED, Some("ticket"))
    );
    listen(&s, &form_key).await;
    let (_, tech) = technician(&s).await;
    let submit = format!("/api/v1/forms/{}/submissions", form["id"].as_str().unwrap());

    let (status, submitted) = s
        .post(
            &tech,
            &submit,
            json!({ "orgId": client_id, "answers": { "name": "Ada", "reason": "New phone" },
                                       "context": { "ticketId": ticket["id"] } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{submitted}");
    let run: Uuid = submitted["runs"][0].as_str().unwrap().parse().unwrap();
    let trigger: Value = sqlx::query_scalar("SELECT trigger FROM runs WHERE id = $1")
        .bind(run)
        .fetch_one(&s.pools.owner)
        .await
        .unwrap();
    let context = &trigger["context"];
    assert_eq!(context["client"]["id"], json!(client_id));
    assert_eq!(context["ticket"]["reference"], ticket["reference"]);
    // The ticket's requester comes along as the contact.
    assert_eq!(
        (
            context["contact"]["email"].as_str(),
            context["contact"]["first_name"].as_str()
        ),
        (Some(email.as_str()), Some("Ada"))
    );
    // The ticket's history says so.
    let (_, activity) = s
        .get(
            &s.owner,
            &format!(
                "/api/v1/tickets/{}/activity",
                ticket["id"].as_str().unwrap()
            ),
        )
        .await;
    assert!(
        activity
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "action" && a["details"]["form"] == "Reset MFA"),
        "{activity}"
    );

    // A ticket or contact of another organization isn't accepted.
    for (bad, pointer) in [
        (json!({ "ticketId": elsewhere["id"] }), "/context/ticketId"),
        (json!({ "contactId": Uuid::now_v7() }), "/context/contactId"),
    ] {
        let (status, problem) = s
            .post(
                &tech,
                &submit,
                json!({ "orgId": client_id, "answers": {}, "context": bad }),
            )
            .await;
        assert_eq!(
            (status, problem["errors"][0]["pointer"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(pointer)),
            "{problem}"
        );
    }
}
