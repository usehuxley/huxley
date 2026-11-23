//! `script` steps: the task waits for a runner, the runner's report
//! settles the step, and failures, retries and timeouts behave.

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{IsolatedDb, TestPools, ensure_root, isolated};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::script::{self, Report, ReportError};
use huxley_engine::{Engine, RunStatus, StartRun};
use huxley_scripts::{Language, NewScript};
use serde_json::{Value as Json, json};
use uuid::Uuid;

struct Harness {
    _db: IsolatedDb,
    pools: TestPools,
    crypto: huxley_crypto::Crypto,
    engine: Engine,
    org: Uuid,
    runner: Uuid,
}

async fn harness() -> Option<Harness> {
    let db = isolated("scripts").await?;
    let pools = db.pools.clone();
    let org = ensure_root(&pools).await;
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = Engine::new(pools.app.clone(), crypto.clone());
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    huxley_scripts::create(
        &mut tx,
        &ctx,
        org,
        &NewScript {
            key: "greet".into(),
            name: "Greet".into(),
            description: None,
            language: Language::Python,
            source: "result = {'greeting': 'hi ' + inputs['name']}".into(),
            parameters: Some(json!({
                "type": "object", "required": ["name"],
                "properties": { "name": { "type": "string" } }
            })),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let runner = Uuid::now_v7();
    sqlx::query("INSERT INTO runners (id, name, source) VALUES ($1, $2, 'huxley')")
        .bind(runner)
        .bind(format!("r-{}", runner.simple()))
        .execute(&pools.owner)
        .await
        .unwrap();
    Some(Harness {
        _db: db,
        pools,
        crypto,
        engine,
        org,
        runner,
    })
}

impl Harness {
    async fn start(&self, step: &str) -> Uuid {
        let yaml = format!(
            r#"apiVersion: usehuxley.com/v1
kind: Workflow
metadata: {{ key: s, name: Script }}
spec:
  steps:
    - {step}
  outputs:
    greeting: "{{{{ steps.run.output.greeting }}}}"
"#
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

    async fn run(&self, id: Uuid) -> (RunStatus, Option<Json>, Option<Json>) {
        sqlx::query_as("SELECT status, outputs, error FROM runs WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }

    async fn step_status(&self, id: Uuid) -> String {
        sqlx::query_scalar(
            "SELECT status::text FROM step_runs WHERE run_id = $1 AND step_key = 'run' \
              ORDER BY attempt DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pools.owner)
        .await
        .unwrap()
        .unwrap_or_else(|| "missing".into())
    }

    async fn until(&self, what: &str, done: impl AsyncFn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done().await {
            assert!(Instant::now() < deadline, "{what} didn't happen");
            self.engine.drain("test", 20).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn claim(&self) -> Option<script::ClaimedTask> {
        self.engine
            .claim_script(self.runner, &[Language::Python])
            .await
            .unwrap()
    }

    async fn claimed(&self) -> script::ClaimedTask {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("test", 20).await.unwrap();
            if let Some(task) = self.claim().await {
                return task;
            }
            assert!(Instant::now() < deadline, "no task was queued");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn ok(output: Json) -> Report {
    Report {
        succeeded: true,
        output: Some(output),
        error: None,
        exit_code: Some(0),
        logs: "done".into(),
    }
}

#[tokio::test]
async fn a_runners_result_completes_the_step() {
    let Some(h) = harness().await else { return };
    let run = h
        .start(r#"{ id: run, type: script, script: greet, with: { name: "{{ 'ada' }}" } }"#)
        .await;
    let task = h.claimed().await;
    assert_eq!(task.language, Language::Python);
    assert!(task.source.contains("greeting"));
    assert_eq!(task.inputs, json!({ "name": "ada" }));
    assert_eq!(h.step_status(run).await, "waiting");
    // Inputs are stored encrypted.
    let sealed: Vec<u8> =
        sqlx::query_scalar("SELECT inputs_sealed FROM script_tasks WHERE id = $1")
            .bind(task.id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("ada"));
    // Nobody else gets it, and only its runner can report.
    assert!(h.claim().await.is_none());
    let stranger = h
        .engine
        .report_script(Uuid::now_v7(), task.id, &ok(json!({})))
        .await;
    assert!(matches!(stranger, Err(ReportError::NotYours)));

    h.engine
        .report_script(h.runner, task.id, &ok(json!({ "greeting": "hi ada" })))
        .await
        .unwrap();
    h.until("the run finishing", async || {
        h.run(run).await.0 == RunStatus::Succeeded
    })
    .await;
    assert_eq!(h.run(run).await.1, Some(json!({ "greeting": "hi ada" })));
    // Reported once.
    let again = h
        .engine
        .report_script(h.runner, task.id, &ok(json!({})))
        .await;
    assert!(matches!(again, Err(ReportError::NotYours)));
}

#[tokio::test]
async fn a_failed_script_is_retried_then_fails_the_run() {
    let Some(h) = harness().await else { return };
    let run = h
        .start(
            "{ id: run, type: script, script: greet, with: { name: ada }, \
               retry: { max_attempts: 2, delay: 10ms } }",
        )
        .await;
    for _ in 0..2 {
        let task = h.claimed().await;
        let report = Report {
            succeeded: false,
            output: None,
            error: Some("the script exited with code 3".into()),
            exit_code: Some(3),
            logs: "boom".into(),
        };
        h.engine
            .report_script(h.runner, task.id, &report)
            .await
            .unwrap();
    }
    h.until("the run failing", async || {
        h.run(run).await.0 == RunStatus::Failed
    })
    .await;
    let attempts: i64 = sqlx::query_scalar("SELECT count(*) FROM script_tasks WHERE run_id = $1")
        .bind(run)
        .fetch_one(&h.pools.owner)
        .await
        .unwrap();
    assert_eq!(attempts, 2);
    let error = h.run(run).await.2.unwrap().to_string();
    assert!(
        error.contains("boom") && error.contains("code 3"),
        "{error}"
    );
}

#[tokio::test]
async fn bad_inputs_and_unknown_scripts_fail_without_a_task() {
    let Some(h) = harness().await else { return };
    let run = h
        .start("{ id: run, type: script, script: greet, with: { nom: ada } }")
        .await;
    h.until("the run failing", async || {
        h.run(run).await.0 == RunStatus::Failed
    })
    .await;
    let error = h.run(run).await.2.unwrap().to_string();
    assert!(error.contains("name"), "{error}");
    let run = h.start("{ id: run, type: script, script: nope }").await;
    h.until("the run failing", async || {
        h.run(run).await.0 == RunStatus::Failed
    })
    .await;
    assert!(h.claim().await.is_none());
}

#[tokio::test]
async fn scripts_nobody_runs_time_out() {
    let Some(h) = harness().await else { return };
    let run = h
        .start("{ id: run, type: script, script: greet, with: { name: ada }, timeout: 1s }")
        .await;
    h.until("the run failing", async || {
        h.run(run).await.0 == RunStatus::Failed
    })
    .await;
    let error = h.run(run).await.2.unwrap().to_string();
    assert!(error.contains("no runner picked"), "{error}");
    // An expired task isn't handed out.
    assert!(h.claim().await.is_none());
}

#[tokio::test]
async fn a_cancelled_runs_script_isnt_handed_out() {
    let Some(h) = harness().await else { return };
    let run = h
        .start("{ id: run, type: script, script: greet, with: { name: ada } }")
        .await;
    h.until("the task being queued", async || {
        h.step_status(run).await == "waiting"
    })
    .await;
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    assert!(h.engine.cancel_run(&mut tx, run).await.unwrap());
    tx.commit().await.unwrap();
    assert!(h.claim().await.is_none());
    h.until("the run being cancelled", async || {
        h.run(run).await.0 == RunStatus::Cancelled
    })
    .await;
    assert!(h.claim().await.is_none());
}

impl Harness {
    async fn secret(&self, key: &str, value: &str) {
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let input = huxley_variables::VariableInput {
            kind: huxley_variables::VariableType::Secret,
            value: json!(value),
            description: None,
            locked: false,
        };
        huxley_variables::set(&mut tx, &ctx, &self.crypto, self.org, key, &input)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    /// An integration `vault` with an API key connection.
    async fn api_key_connection(&self, key: &str) {
        // The org's first data key audits in a transaction of its own:
        // make it before this one holds the audit chain.
        let binding = huxley_crypto::Binding {
            purpose: "test",
            record: self.org,
        };
        self.crypto.encrypt(self.org, binding, b"x").await.unwrap();
        let spec = r"openapi: 3.0.3
info: { title: Vault, version: '1' }
servers: [ { url: 'https://vault.example.com/api' } ]
components:
  securitySchemes:
    key: { type: apiKey, in: header, name: X-Key }
paths: {}
";
        let ctx = RequestContext::system(Via::System);
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let integration =
            huxley_integrations::store::create(&mut tx, &ctx, self.org, Some("vault"), spec)
                .await
                .unwrap();
        let input = huxley_integrations::connections::ConnectionInput {
            name: "Main".into(),
            key: Some("main".into()),
            auth_scheme: Some("key".into()),
            secrets: [("key".to_owned(), key.to_owned())].into(),
            ..huxley_integrations::connections::ConnectionInput::default()
        };
        huxley_integrations::connections::create(
            &mut tx,
            &self.crypto,
            &ctx,
            self.org,
            integration.summary.id,
            &input,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

#[tokio::test]
async fn credentials_reach_the_script_and_are_scrubbed_from_what_comes_back() {
    let Some(h) = harness().await else { return };
    h.secret("db_password", "pw-HUNTER-2026").await;
    h.api_key_connection("sk-live-ABCDEF").await;
    let run = h
        .start(
            r#"{ id: run, type: script, script: greet, with: { name: ada },
                 env: { DB_PASSWORD: "{{ secrets.db_password }}", REGION: eu },
                 connections: { VAULT: vault/main } }"#,
        )
        .await;
    let task = h.claimed().await;
    assert_eq!(task.env["DB_PASSWORD"], "pw-HUNTER-2026");
    assert_eq!(task.env["REGION"], "eu");
    assert_eq!(task.env["VAULT_URL"], "https://vault.example.com/api");
    assert_eq!(task.env["VAULT_KEY"], "sk-live-ABCDEF");
    let mut redact = task.redact.clone();
    redact.sort();
    assert_eq!(redact, vec!["pw-HUNTER-2026", "sk-live-ABCDEF"]);
    // Not in the parameters, and not stored in the clear.
    assert_eq!(task.inputs, json!({ "name": "ada" }));
    let stored: (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT inputs_sealed, redact_sealed FROM script_tasks WHERE id = $1")
            .bind(task.id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    for sealed in [stored.0, stored.1] {
        let text = String::from_utf8_lossy(&sealed).into_owned();
        assert!(!text.contains("HUNTER") && !text.contains("ABCDEF"));
    }

    // A careless runner echoes them back: Huxley scrubs them anyway.
    let report = Report {
        succeeded: true,
        output: Some(json!({ "greeting": "key sk-live-ABCDEF, pw pw-HUNTER-2026" })),
        error: None,
        exit_code: Some(0),
        logs: "connecting with sk-live-ABCDEF".into(),
    };
    h.engine
        .report_script(h.runner, task.id, &report)
        .await
        .unwrap();
    h.until("the run finishing", async || {
        h.run(run).await.0 == RunStatus::Succeeded
    })
    .await;
    let outputs = h.run(run).await.1.unwrap().to_string();
    assert!(
        !outputs.contains("ABCDEF") && !outputs.contains("HUNTER"),
        "{outputs}"
    );
    let (logs, output): (String, Json) =
        sqlx::query_as("SELECT logs, output FROM script_tasks WHERE id = $1")
            .bind(task.id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();
    assert!(!logs.contains("ABCDEF"), "{logs}");
    assert!(!output.to_string().contains("ABCDEF"), "{output}");
}

#[tokio::test]
async fn a_missing_connection_fails_the_step() {
    let Some(h) = harness().await else { return };
    let run = h
        .start("{ id: run, type: script, script: greet, with: { name: ada }, connections: { X: nope } }")
        .await;
    h.until("the run failing", async || {
        h.run(run).await.0 == RunStatus::Failed
    })
    .await;
    let error = h.run(run).await.2.unwrap().to_string();
    assert!(error.contains("nope"), "{error}");
    assert!(h.claim().await.is_none());
}

#[tokio::test]
async fn running_scripts_output_is_appended_scrubbed_then_replaced() {
    let Some(h) = harness().await else { return };
    h.api_key_connection("sk-live-ABCDEF").await;
    let run = h
        .start("{ id: run, type: script, script: greet, with: { name: ada }, connections: { VAULT: vault } }")
        .await;
    let task = h.claimed().await;
    let logs = async || -> Option<String> {
        sqlx::query_scalar("SELECT logs FROM script_tasks WHERE id = $1")
            .bind(task.id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap()
    };
    h.engine
        .append_script_logs(h.runner, task.id, "using sk-live-ABCDEF\n")
        .await
        .unwrap();
    h.engine
        .append_script_logs(h.runner, task.id, "step two\n")
        .await
        .unwrap();
    assert_eq!(
        logs().await.as_deref(),
        Some("using «redacted»\nstep two\n")
    );
    // Only its runner, only while it runs.
    let stranger = h
        .engine
        .append_script_logs(Uuid::now_v7(), task.id, "x")
        .await;
    assert!(matches!(stranger, Err(ReportError::NotYours)));
    // The whole log arrives with the report.
    h.engine
        .report_script(h.runner, task.id, &ok(json!({ "greeting": "hi" })))
        .await
        .unwrap();
    assert_eq!(logs().await.as_deref(), Some("done"));
    let late = h.engine.append_script_logs(h.runner, task.id, "late").await;
    assert!(matches!(late, Err(ReportError::NotYours)));
    h.until("the run finishing", async || {
        h.run(run).await.0 == RunStatus::Succeeded
    })
    .await;
}
