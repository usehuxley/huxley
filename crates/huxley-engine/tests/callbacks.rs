//! `wait_for_callback` steps: the URL is handed over through `send`, and a
//! call to it resumes the run.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::extract::State;
use axum::routing::post;
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{CallbackRequest, EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

type Seen = Arc<Mutex<Vec<Json>>>;

async fn start(
    State(seen): State<Seen>,
    AxumJson(body): AxumJson<Json>,
) -> (axum::http::StatusCode, AxumJson<Json>) {
    let mut seen = seen.lock().unwrap();
    seen.push(body.clone());
    // `flaky` fails the first time.
    if body["flaky"] == true && seen.len() == 1 {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            AxumJson(json!({})),
        );
    }
    (
        axum::http::StatusCode::OK,
        AxumJson(json!({ "queued": true })),
    )
}

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    org: Uuid,
    port: u16,
    seen: Seen,
}

async fn harness(public_url: Option<&str>) -> Option<Harness> {
    let db = isolated("callbacks").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/start", post(start))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::with_config(pools.app.clone(), crypto, &config)
        .with_public_url(public_url.map(str::to_owned));
    Some(Harness {
        _db: db,
        pools,
        engine,
        org,
        port,
        seen,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    status: RunStatus,
    outputs: Option<Json>,
    error: Option<Json>,
}

impl Harness {
    async fn start(&self, expires: &str) -> Uuid {
        self.start_with(expires, false).await
    }

    async fn start_with(&self, expires: &str, flaky: bool) -> Uuid {
        let yaml = format!(
            r#"apiVersion: usehuxley.com/v1
kind: Workflow
metadata: {{ key: cb, name: Callback }}
spec:
  steps:
    - id: rebooted
      type: wait_for_callback
      expires: {expires}
      retry: {{ max_attempts: 2, delay: 10ms }}
      send:
        request:
          method: POST
          url: "http://localhost:{port}/start"
          body: {{ script: reboot, flaky: {flaky}, callback: "{{{{ callback.url }}}}" }}
  outputs:
    status: "{{{{ steps.rebooted.output.body.status }}}}"
    method: "{{{{ steps.rebooted.output.method }}}}"
"#,
            port = self.port
        );
        let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let id = self
            .engine
            .start_run(
                &mut tx,
                StartRun {
                    org_id: self.org,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs: json!({}),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn run(&self, id: Uuid) -> RunRow {
        sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }

    async fn step(&self, id: Uuid) -> (String, Option<Json>, Option<Json>) {
        let row: Option<(String, Option<Json>, Option<Json>)> = sqlx::query_as(
            "SELECT status::text, input, error FROM step_runs WHERE run_id = $1 AND step_key = 'rebooted' ORDER BY attempt DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pools.owner)
        .await
        .unwrap();
        // Not started yet.
        row.unwrap_or_else(|| ("missing".into(), None, None))
    }

    /// Drains jobs until `done` says so.
    async fn until(&self, what: &str, done: impl AsyncFn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done().await {
            assert!(Instant::now() < deadline, "{what} didn't happen");
            self.engine.drain("test", 20).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn token_of(url: &str) -> String {
    url.rsplit('/').next().unwrap().to_owned()
}

#[tokio::test]
async fn a_callback_resumes_the_run_once() {
    let Some(h) = harness(Some("https://huxley.example.com/")).await else {
        return;
    };
    let id = h.start("1h").await;
    h.until("the send", async || !h.seen.lock().unwrap().is_empty())
        .await;
    h.until("the step parking", async || h.step(id).await.0 == "waiting")
        .await;

    let sent = h.seen.lock().unwrap()[0].clone();
    assert_eq!(sent["script"], "reboot");
    let url = sent["callback"].as_str().unwrap();
    assert!(
        url.starts_with("https://huxley.example.com/callbacks/"),
        "{url}"
    );
    assert_eq!(h.run(id).await.status, RunStatus::Running);
    // The token is a credential: the step's input is kept sealed.
    let (_, input, _) = h.step(id).await;
    assert_eq!(input, Some(json!(huxley_expr::REDACTED)));

    let call = || CallbackRequest {
        method: "POST".into(),
        query: json!({}),
        content_type: Some("application/json".into()),
        body: json!({ "status": "done" }),
    };
    assert!(
        h.engine
            .complete_callback(&token_of(url), call())
            .await
            .unwrap()
    );
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    let run = h.run(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({ "status": "done", "method": "POST" })
    );
    // Once only, and unknown tokens are refused the same way.
    assert!(
        !h.engine
            .complete_callback(&token_of(url), call())
            .await
            .unwrap()
    );
    assert!(
        !h.engine
            .complete_callback("not-a-token", call())
            .await
            .unwrap()
    );
    assert_eq!(h.seen.lock().unwrap().len(), 1, "sent once");
}

#[tokio::test]
async fn callbacks_nobody_calls_time_out() {
    let Some(h) = harness(Some("https://huxley.example.com")).await else {
        return;
    };
    let id = h.start("1s").await;
    h.until("the step parking", async || h.step(id).await.0 == "waiting")
        .await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    let run = h.run(id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert_eq!(h.step(id).await.2.unwrap()["kind"], "timeout");
    // Too late now.
    let url = h.seen.lock().unwrap()[0]["callback"]
        .as_str()
        .unwrap()
        .to_owned();
    let call = CallbackRequest {
        method: "POST".into(),
        query: json!({}),
        content_type: None,
        body: Json::Null,
    };
    assert!(
        !h.engine
            .complete_callback(&token_of(&url), call)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn callbacks_need_a_public_url() {
    let Some(h) = harness(None).await else { return };
    let id = h.start("1h").await;
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    let run = h.run(id).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("auth.public_url")
    );
    assert!(h.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_retried_send_keeps_the_url_working() {
    let Some(h) = harness(Some("https://huxley.example.com")).await else {
        return;
    };
    let id = h.start_with("1h", true).await;
    h.until("two sends", async || h.seen.lock().unwrap().len() == 2)
        .await;
    h.until("the step parking", async || h.step(id).await.0 == "waiting")
        .await;
    let urls: Vec<String> = h
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|b| b["callback"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(urls[0], urls[1], "the retry sends the same URL");
    let call = CallbackRequest {
        method: "POST".into(),
        query: json!({}),
        content_type: None,
        body: json!({ "status": "done" }),
    };
    assert!(
        h.engine
            .complete_callback(&token_of(&urls[1]), call)
            .await
            .unwrap()
    );
    h.until("the run finishing", async || {
        h.run(id).await.status.is_finished()
    })
    .await;
    assert_eq!(h.run(id).await.status, RunStatus::Succeeded);
}
