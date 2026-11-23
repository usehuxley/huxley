//! `http.request` and `core.notify` against a mock HTTP server and a real
//! PostgreSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Json as AxumJson;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{EgressConfig, Engine, EngineConfig, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default)]
struct Mock {
    flaky_calls: AtomicUsize,
}

async fn echo(headers: HeaderMap, Query(query): Query<HashMap<String, String>>) -> AxumJson<Json> {
    AxumJson(json!({
        "query": query,
        "auth": headers.get("authorization").and_then(|v| v.to_str().ok()),
    }))
}

async fn form(Form(fields): Form<HashMap<String, String>>) -> String {
    format!("got {}", fields.get("name").cloned().unwrap_or_default())
}

async fn flaky(State(mock): State<Arc<Mock>>) -> impl IntoResponse {
    let n = mock.flaky_calls.fetch_add(1, Ordering::SeqCst) + 1;
    if n < 3 {
        (StatusCode::SERVICE_UNAVAILABLE, "try later").into_response()
    } else {
        AxumJson(json!({ "attempt": n })).into_response()
    }
}

async fn missing() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        AxumJson(json!({ "error": "no such thing" })),
    )
}

async fn slow() -> &'static str {
    tokio::time::sleep(Duration::from_secs(3)).await;
    "late"
}

async fn big() -> String {
    "x".repeat(4096)
}

async fn to_metadata() -> Redirect {
    Redirect::temporary("http://169.254.169.254/latest/meta-data")
}

async fn mock_server() -> (u16, Arc<Mock>) {
    let mock = Arc::new(Mock::default());
    let app = axum::Router::new()
        .route("/echo", get(echo))
        .route("/form", post(form))
        .route("/flaky", get(flaky))
        .route("/missing", get(missing))
        .route("/slow", get(slow))
        .route("/big", get(big))
        .route("/redirect", get(to_metadata))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, mock)
}

struct Harness {
    pools: TestPools,
    engine: Engine,
    org: Uuid,
}

/// `localhost` may reach the mock server; nothing else private may.
async fn harness(max_response_bytes: usize) -> Option<Harness> {
    let pools = pools().await?;
    let root = ensure_root(&pools).await;
    let org = Uuid::now_v7();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) \
         VALUES ($1, $2, 'provider', $3, '')",
    )
    .bind(org)
    .bind(root)
    .bind(unique("steps"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let config = EngineConfig {
        egress: EgressConfig {
            private_hosts: vec!["localhost".into()],
            max_response_bytes,
            ..EgressConfig::default()
        },
        ..EngineConfig::default()
    };
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::with_config(pools.app.clone(), crypto, &config);
    Some(Harness { pools, engine, org })
}

impl Harness {
    async fn run(&self, spec: &str) -> (Uuid, RunStatus, Option<Json>, Option<Json>) {
        let yaml = format!(
            "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n{spec}"
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
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("steps-test", 50).await.unwrap();
            let (status, outputs, error): (RunStatus, Option<Json>, Option<Json>) =
                sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
                    .bind(id)
                    .fetch_one(&self.pools.owner)
                    .await
                    .unwrap();
            if status.is_finished() {
                return (id, status, outputs, error);
            }
            assert!(Instant::now() < deadline, "run didn't finish");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn attempts(&self, id: Uuid, key: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM step_runs WHERE run_id = $1 AND step_key = $2")
            .bind(id)
            .bind(key)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn requests_and_responses() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (port, _) = mock_server().await;
    let (_, status, outputs, error) = h
        .run(&format!(
            r#"  steps:
    - id: call
      type: http.request
      with:
        url: http://localhost:{port}/echo
        query: {{ q: "a b", tags: [x, y] }}
        headers: {{ Authorization: "Bearer {{{{ 'tok' ~ 'en' }}}}" }}
    - id: post
      type: http.request
      with: {{ method: POST, url: "http://localhost:{port}/form", body_type: form, body: {{ name: Ada }} }}
  outputs:
    status: "{{{{ steps.call.output.status }}}}"
    query: "{{{{ steps.call.output.body.query.q }}}}"
    auth: "{{{{ steps.call.output.body.auth }}}}"
    type: "{{{{ steps.call.output.headers['content-type'] }}}}"
    form: "{{{{ steps.post.output.body }}}}"
"#
        ))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(
        outputs.unwrap(),
        json!({
            "status": 200,
            "query": "a b",
            "auth": "Bearer token",
            "type": "application/json",
            "form": "got Ada",
        })
    );
}

#[tokio::test]
async fn retries_follow_the_policy() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (port, mock) = mock_server().await;
    let (id, status, outputs, error) = h
        .run(&format!(
            r#"  steps:
    - id: call
      type: http.request
      with: {{ url: "http://localhost:{port}/flaky" }}
      retry: {{ max_attempts: 4, delay: 10ms, on: [5xx] }}
  outputs: {{ attempt: "{{{{ steps.call.output.body.attempt }}}}" }}
"#
        ))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap(), json!({ "attempt": 3 }));
    assert_eq!(mock.flaky_calls.load(Ordering::SeqCst), 3);
    assert_eq!(h.attempts(id, "call").await, 3);

    // A 404 doesn't match `5xx`: one attempt, and the run fails with it.
    let (id, status, _, error) = h
        .run(&format!(
            r#"  steps:
    - id: call
      type: http.request
      with: {{ url: "http://localhost:{port}/missing" }}
      retry: {{ max_attempts: 3, delay: 10ms, on: [5xx] }}
"#
        ))
        .await;
    assert_eq!(status, RunStatus::Failed);
    let error = error.unwrap();
    assert_eq!(error["kind"], "http");
    assert_eq!(error["status"], 404);
    assert_eq!(error["body"]["error"], "no such thing");
    assert_eq!(h.attempts(id, "call").await, 1);
}

#[tokio::test]
async fn egress_policy_blocks_internal_addresses() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (port, _) = mock_server().await;
    for (url, why) in [
        (
            format!("http://127.0.0.1:{port}/echo"),
            "isn't a public address",
        ),
        (
            "http://169.254.169.254/latest/meta-data".to_owned(),
            "isn't a public address",
        ),
        (
            format!("http://localhost:{port}/redirect"),
            "169.254.169.254",
        ),
        ("file:///etc/passwd".to_owned(), "only http and https"),
    ] {
        let (_, status, _, error) = h
            .run(&format!(
                "  steps:\n    - {{ id: call, type: http.request, with: {{ url: \"{url}\" }}, retry: {{ max_attempts: 3, delay: 10ms }} }}\n"
            ))
            .await;
        assert_eq!(status, RunStatus::Failed, "{url}");
        let error = error.unwrap();
        assert_eq!(error["kind"], "egress", "{url}: {error}");
        assert!(
            error["message"].as_str().unwrap().contains(why),
            "{url}: {error}"
        );
    }
}

#[tokio::test]
async fn timeouts_and_size_limits() {
    let Some(h) = harness(1024).await else { return };
    let (port, _) = mock_server().await;
    let (_, status, _, error) = h
        .run(&format!(
            "  steps:\n    - {{ id: call, type: http.request, timeout: 200ms, with: {{ url: \"http://localhost:{port}/slow\" }} }}\n"
        ))
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.unwrap()["kind"], "timeout");

    let (_, status, _, error) = h
        .run(&format!(
            "  steps:\n    - {{ id: call, type: http.request, with: {{ url: \"http://localhost:{port}/big\" }} }}\n"
        ))
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.unwrap()["kind"], "limit");
}

#[tokio::test]
async fn notify_reaches_users_teams_and_roles() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let user = |name: &str| {
        let id = Uuid::now_v7();
        (id, format!("{}-{}@example.com", name, id.simple()))
    };
    let (alice, alice_email) = user("alice");
    let (bob, bob_email) = user("bob");
    for (id, email) in [(alice, &alice_email), (bob, &bob_email)] {
        sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'Test')")
            .bind(id)
            .bind(email)
            .execute(&h.pools.owner)
            .await
            .unwrap();
    }
    // Bob holds a role of this test's own on the run's org.
    let role = format!("notify_{}", &Uuid::now_v7().simple().to_string()[24..]);
    let role_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO roles (id, key, name, description, is_system, permissions) \
         VALUES ($1, $2, 'Notify test', '', false, '{}')",
    )
    .bind(role_id)
    .bind(&role)
    .execute(&h.pools.owner)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(bob)
    .bind(role_id)
    .bind(h.org)
    .execute(&h.pools.owner)
    .await
    .unwrap();

    let (id, status, _, error) = h
        .run(&format!(
            r#"  steps:
    - id: tell
      type: core.notify
      with: {{ to: ["user:{alice}", "role:{role}", "user:{bob}"], title: "Run {{{{ run.workflow.key }}}} done", url: /runs }}
"#
        ))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    let output: Json =
        sqlx::query_scalar("SELECT output FROM step_runs WHERE run_id = $1 AND step_key = 'tell'")
            .bind(id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    // Bob counted once even though he's named twice.
    assert_eq!(output["sent"], 2);
    let titles: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, title, link FROM notifications WHERE user_id = ANY($1) ORDER BY title",
    )
    .bind(vec![alice, bob])
    .fetch_all(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(titles.len(), 2);
    assert!(
        titles
            .iter()
            .all(|t| t.1 == "Run t done" && t.2.as_deref() == Some("/runs"))
    );

    let (_, status, _, error) = h
        .run("  steps:\n    - { id: tell, type: core.notify, with: { to: \"someone\", title: x } }\n")
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.unwrap()["kind"], "definition");
}

#[tokio::test]
async fn variables_and_secrets_reach_steps_but_not_the_record() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (port, _) = mock_server().await;
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    let crypto =
        huxley_crypto::Crypto::new(h.pools.app.clone(), huxley_crypto::testing::key_ring());
    for (key, kind, value) in [
        (
            "path",
            huxley_variables::VariableType::String,
            json!("echo"),
        ),
        (
            "api_token",
            huxley_variables::VariableType::Secret,
            json!("tok-SECRET-42"),
        ),
    ] {
        let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
        let input = huxley_variables::VariableInput {
            kind,
            value,
            description: None,
            locked: false,
        };
        huxley_variables::set(&mut tx, &ctx, &crypto, h.org, key, &input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let (id, status, outputs, error) = h
        .run(&format!(
            r#"  steps:
    - id: call
      type: http.request
      with:
        url: "http://localhost:{port}/{{{{ vars.path }}}}"
        headers: {{ Authorization: "Bearer {{{{ secrets.api_token }}}}" }}
  outputs: {{ auth: "{{{{ steps.call.output.body.auth }}}}" }}
"#
        ))
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    // The server got the real token; the record and outputs never hold it.
    assert_eq!(outputs.unwrap(), json!({ "auth": "Bearer «redacted»" }));
    let (input, output, tainted): (Json, Json, bool) = sqlx::query_as(
        "SELECT input, output, tainted FROM step_runs WHERE run_id = $1 AND step_key = 'call'",
    )
    .bind(id)
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert!(tainted);
    assert_eq!(input, json!("«redacted»"));
    assert_eq!(output["body"]["auth"], "Bearer «redacted»");
    let everything: String = sqlx::query_scalar(
        "SELECT string_agg(coalesce(input::text, '') || coalesce(output::text, ''), '') \
           FROM step_runs WHERE run_id = $1",
    )
    .bind(id)
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert!(!everything.contains("tok-SECRET-42"));
}
