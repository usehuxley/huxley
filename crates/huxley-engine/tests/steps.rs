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
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated, unique};
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
    _db: IsolatedDb,
    pools: TestPools,
    engine: Engine,
    org: Uuid,
    /// What `core.email` steps sent.
    mail: Arc<std::sync::Mutex<Vec<huxley_email::Email>>>,
}

/// `localhost` may reach the mock server; nothing else private may. Each
/// test has a database of its own: other test processes run engines with
/// different egress settings, and must not pick up this test's jobs.
async fn harness(max_response_bytes: usize) -> Option<Harness> {
    let db = isolated("steps").await?;
    let pools = db.pools.clone();
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
    let (mailer, mail) = huxley_email::Mailer::memory();
    let engine = Engine::with_config(pools.app.clone(), crypto, &config).with_mailer(mailer);
    Some(Harness {
        _db: db,
        pools,
        engine,
        org,
        mail,
    })
}

impl Harness {
    async fn run(&self, spec: &str) -> (Uuid, RunStatus, Option<Json>, Option<Json>) {
        self.run_with(spec, json!({})).await
    }

    async fn run_with(
        &self,
        spec: &str,
        inputs: Json,
    ) -> (Uuid, RunStatus, Option<Json>, Option<Json>) {
        self.run_in(self.org, spec, inputs).await
    }

    /// Runs `spec` as a workflow of organization `org`.
    async fn run_in(
        &self,
        org: Uuid,
        spec: &str,
        inputs: Json,
    ) -> (Uuid, RunStatus, Option<Json>, Option<Json>) {
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
                    org_id: org,
                    workflow_id: None,
                    version: None,
                    document: Some(document),
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs,
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

#[tokio::test]
async fn secret_inputs_are_sealed_and_taint_what_reads_them() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (port, _) = mock_server().await;
    let (id, status, outputs, error) = h
        .run_with(
            &format!(
                r#"  inputs:
    type: object
    properties:
      name: {{ type: string }}
      password: {{ type: string, writeOnly: true }}
  steps:
    - id: greet
      type: transform
      output: "hi {{{{ inputs.name }}}}"
    - id: call
      type: http.request
      with:
        url: "http://localhost:{port}/echo"
        headers: {{ Authorization: "Basic {{{{ inputs.password }}}}" }}
  outputs:
    greeting: "{{{{ steps.greet.output }}}}"
    auth: "{{{{ steps.call.output.body.auth }}}}"
"#
            ),
            json!({ "name": "ada", "password": "pw-HUNTER-2026" }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    // The server got it; the record, the outputs and the run's inputs don't.
    assert_eq!(
        outputs.unwrap(),
        json!({ "greeting": "hi ada", "auth": "Basic «redacted»" })
    );
    let (inputs, sealed): (Json, Option<Vec<u8>>) =
        sqlx::query_as("SELECT inputs, inputs_sealed FROM runs WHERE id = $1")
            .bind(id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    assert_eq!(inputs, json!({ "name": "ada", "password": "«redacted»" }));
    assert!(!String::from_utf8_lossy(&sealed.unwrap()).contains("HUNTER"));
    let rows: Vec<(String, bool, String)> = sqlx::query_as(
        "SELECT step_key, tainted, coalesce(input::text, '') || coalesce(output::text, '') \
           FROM step_runs WHERE run_id = $1 ORDER BY step_key",
    )
    .bind(id)
    .fetch_all(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(rows[0].0, "call");
    assert!(rows[0].1, "reading the secret input taints the step");
    assert!(!rows[1].1, "reading other inputs doesn't");
    assert!(rows.iter().all(|r| !r.2.contains("HUNTER")));
}

#[tokio::test]
async fn email_steps_send_one_copy_per_recipient() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (_, status, outputs, error) = h
        .run_with(
            r#"  steps:
    - id: mail
      type: core.email
      with:
        to: ["{{ inputs.who }}", ops@example.com]
        subject: "Welcome {{ inputs.name }}"
        text: "Hi {{ inputs.name }},\nyour account is ready."
  outputs: { sent: "{{ steps.mail.output.sent }}" }
"#,
            json!({ "who": "ada@example.com", "name": "Ada <script>" }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap()["sent"], 2);
    let mail = h.mail.lock().unwrap().clone();
    assert_eq!(mail.len(), 2);
    assert_eq!(mail[0].to, "ada@example.com");
    assert_eq!(mail[1].to, "ops@example.com");
    assert_eq!(mail[0].subject, "Welcome Ada <script>");
    // Without an HTML body, the text is escaped into one.
    assert!(
        mail[0].html.contains("Ada &lt;script&gt;,<br>your account"),
        "{}",
        mail[0].html
    );

    // A header smuggled into an address is refused, and nothing is sent.
    let (_, status, _, error) = h
        .run_with(
            r#"  steps:
    - { id: mail, type: core.email, with: { to: "{{ inputs.who }}", subject: Hi, text: x } }
"#,
            json!({ "who": "a@example.com\r\nBcc: x@evil.com" }),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.unwrap()["kind"], "input");
    assert_eq!(h.mail.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn pinned_secret_results_are_resealed_for_the_new_run() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    let crypto =
        huxley_crypto::Crypto::new(h.pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let input = huxley_variables::VariableInput {
        kind: huxley_variables::VariableType::Secret,
        value: json!("tok-PINNED-1"),
        description: None,
        locked: false,
    };
    huxley_variables::set(&mut tx, &ctx, &crypto, h.org, "api_token", &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // `use` fails the run unless it reads the real token (19 characters).
    let spec = r#"  steps:
    - { id: auth, type: transform, output: "Bearer {{ secrets.api_token }}" }
    - id: use
      type: if
      condition: "{{ (steps.auth.output | length) == 19 }}"
      else: [ { id: wrong, type: fail, message: "the pinned value didn't decrypt" } ]
"#;
    let (source, status, _, error) = h.run(spec).await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");

    // A second run of the same document, starting at `use`.
    let yaml = format!(
        "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: t, name: T }}\nspec:\n{spec}"
    );
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
    let steps = huxley_engine::replay::steps_before(&document, "use").unwrap();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let run = h
        .engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: h.org,
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
    let pinned = h
        .engine
        .pin_steps(&mut tx, h.org, run, source, &steps)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(pinned, 1);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        h.engine.drain("steps-test", 50).await.unwrap();
        let (status, error): (RunStatus, Option<Json>) =
            sqlx::query_as("SELECT status, error FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&h.pools.owner)
                .await
                .unwrap();
        if status.is_finished() {
            // `use` read the real (resealed) value.
            assert_eq!(status, RunStatus::Succeeded, "{error:?}");
            break;
        }
        assert!(Instant::now() < deadline, "the pinned run didn't finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A client below the harness's provider, and a users dataset at the
/// provider. Returns the client.
async fn users_dataset(h: &Harness) -> Uuid {
    // A client below the harness's provider, and a users dataset at the provider.
    let client = Uuid::now_v7();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query("INSERT INTO organizations (id, parent_id, kind, name, path) VALUES ($1, $2, 'client', $3, '')")
        .bind(client)
        .bind(h.org)
        .bind(unique("ds client"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let input: huxley_datasets::DatasetInput = serde_json::from_value(json!({
        "key": "users", "name": "Users",
        "columns": [{ "name": "upn", "type": "text" }, { "name": "seats", "type": "integer" }],
        "primaryKey": ["upn"]
    }))
    .unwrap();
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    huxley_datasets::create(&mut tx, &ctx, h.org, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    client
}

#[tokio::test]
async fn dataset_steps_read_and_write_their_subtree() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let client = users_dataset(&h).await;
    // The provider writes rows for itself and its client, then reads them.
    let (_, status, outputs, error) = h
        .run_with(
            r#"  steps:
    - id: theirs
      type: dataset.upsert
      with:
        dataset: users
        org_id: "{{ inputs.client }}"
        rows: [{ upn: ada@client.test, seats: 2 }, { upn: bob@client.test, seats: 1 }]
    - id: ours
      type: dataset.upsert
      with: { dataset: users, rows: "{{ [{'upn': 'tech@msp.test', 'seats': 5}] }}" }
    - id: all
      type: dataset.query
      with: { dataset: users, order_by: "-seats" }
    - id: client_only
      type: dataset.query
      with: { dataset: users, org_id: "{{ inputs.client }}", filters: { seats: 2 } }
  outputs:
    written: "{{ steps.theirs.output.written }}"
    total: "{{ steps.all.output.total }}"
    top: "{{ steps.all.output.rows[0].upn }}"
    match: "{{ steps.client_only.output.rows[0].upn }}"
"#,
            json!({ "client": client }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(
        outputs.unwrap(),
        json!({ "written": 2, "total": 3, "top": "tech@msp.test", "match": "ada@client.test" })
    );

    // A run of the client writes its own rows to its MSP's dataset (row
    // security hides the MSP itself from it), sees only its own rows, and
    // can't write above itself.
    let (_, status, outputs, error) = h
        .run_in(
            client,
            r#"  steps:
    - id: add
      type: dataset.upsert
      with: { dataset: users, rows: [{ upn: cat@client.test }] }
    - id: mine
      type: dataset.query
      with: { dataset: users }
  outputs: { total: "{{ steps.mine.output.total }}" }
"#,
            json!({}),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(outputs.unwrap(), json!({ "total": 3 }));
    let (_, status, _, error) = h
        .run_in(
            client,
            r#"  steps:
    - id: up
      type: dataset.upsert
      with: { dataset: users, org_id: "{{ inputs.provider }}", rows: [{ upn: evil@x }] }
"#,
            json!({ "provider": h.org }),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("one below it"));
}

#[tokio::test]
async fn secrets_never_go_into_datasets() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    users_dataset(&h).await;
    // Secrets never go into a dataset.
    let (_, status, _, error) = h
        .run_with(
            r#"  inputs:
    type: object
    properties: { pin: { type: string, writeOnly: true } }
  steps:
    - id: leak
      type: dataset.upsert
      with: { dataset: users, rows: "{{ [{'upn': inputs.pin}] }}" }
"#,
            json!({ "pin": "1234-SECRET" }),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("secrets"));
    let leaked: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM datasets.ds_{} WHERE upn LIKE '%SECRET%'",
        {
            let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
            huxley_datasets::find(&mut tx, h.org, "users")
                .await
                .unwrap()
                .id
                .simple()
        }
    )))
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(leaked, 0);
}

/// Answers like Anthropic's Messages API: a label from a classification's
/// enum, an extraction's fields, or an echo; counts the calls.
async fn mock_anthropic(calls: Arc<AtomicUsize>) -> u16 {
    let app = axum::Router::new().route(
        "/v1/messages",
        post(move |AxumJson(body): AxumJson<Json>| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let schema = &body["output_config"]["format"]["schema"];
                let text = if let Some(labels) = schema["properties"]["label"]["enum"].as_array() {
                    json!({ "label": labels[1] }).to_string()
                } else if schema.is_object() {
                    assert_eq!(
                        schema["additionalProperties"], false,
                        "extraction schemas are made strict"
                    );
                    json!({ "name": "Ada Lovelace", "start": "2026-12-01" }).to_string()
                } else {
                    format!(
                        "summary of: {}",
                        body["messages"][0]["content"].as_str().unwrap_or_default()
                    )
                };
                AxumJson(json!({ "model": body["model"], "stop_reason": "end_turn",
                                     "content": [{ "type": "text", "text": text }],
                                     "usage": { "input_tokens": 20, "output_tokens": 5 } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

#[tokio::test]
async fn ai_steps_call_the_organizations_provider() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let port = mock_anthropic(calls.clone()).await;
    let crypto =
        huxley_crypto::Crypto::new(h.pools.app.clone(), huxley_crypto::testing::key_ring());
    crypto
        .encrypt(
            h.org,
            huxley_crypto::Binding {
                purpose: "test",
                record: h.org,
            },
            b"x",
        )
        .await
        .unwrap();
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    let input: huxley_ai::store::ProviderInput = serde_json::from_value(json!({
        "kind": "anthropic", "baseUrl": format!("http://localhost:{port}"), "apiKey": "sk-ant-test"
    }))
    .unwrap();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    huxley_ai::store::set(&mut tx, &ctx, &crypto, h.org, &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (id, status, outputs, error) = h
        .run_with(
            r#"  inputs: { type: object, properties: { email: { type: string } } }
  steps:
    - id: gist
      type: ai.prompt
      with: { prompt: "{{ inputs.email }}", system: "Summarize in one line." }
    - id: kind
      type: ai.classify
      with: { text: "{{ inputs.email }}", labels: [billing, onboarding, outage] }
    - id: who
      type: ai.extract
      with:
        text: "{{ inputs.email }}"
        schema: { type: object, properties: { name: { type: string }, start: { type: string, format: date } } }
  outputs:
    gist: "{{ steps.gist.output.text }}"
    kind: "{{ steps.kind.output.label }}"
    name: "{{ steps.who.output.name }}"
"#,
            json!({ "email": "Please set up Ada Lovelace, starting December 1." }),
        )
        .await;
    assert_eq!(status, RunStatus::Succeeded, "{error:?}");
    assert_eq!(
        outputs.unwrap(),
        json!({ "gist": "summary of: Please set up Ada Lovelace, starting December 1.",
                "kind": "onboarding", "name": "Ada Lovelace" })
    );
    let logged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_calls WHERE run_id = $1 AND outcome = 'ok'")
            .bind(id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    assert_eq!(logged, 3);

    // Secrets aren't sent to the provider.
    let before = calls.load(Ordering::SeqCst);
    let (_, status, _, error) = h
        .run_with(
            r#"  inputs: { type: object, properties: { password: { type: string, writeOnly: true } } }
  steps:
    - { id: leak, type: ai.prompt, with: { prompt: "Is {{ inputs.password }} strong?" } }
"#,
            json!({ "password": "hunter2-SECRET" }),
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    assert!(error.unwrap().to_string().contains("secrets"));
    assert_eq!(calls.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn ai_steps_fail_clearly_without_a_provider() {
    let Some(h) = harness(1 << 20).await else {
        return;
    };
    let (_, status, _, error) = h
        .run(
            r#"  steps:
    - { id: ask, type: ai.prompt, with: { prompt: "hello" } }
"#,
        )
        .await;
    assert_eq!(status, RunStatus::Failed);
    let error = error.unwrap().to_string();
    assert!(error.contains("no AI provider"), "{error}");
}
