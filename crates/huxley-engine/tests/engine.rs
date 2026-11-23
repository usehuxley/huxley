//! The engine against a real PostgreSQL. Tests share the database (and its
//! job queue), so each waits for its own run rather than for an empty queue.

use std::time::{Duration, Instant};

use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, RunStatus, StartRun};
use serde_json::{Value as Json, json};
use uuid::Uuid;

struct Harness {
    pools: TestPools,
    engine: Engine,
    org: Uuid,
}

async fn harness() -> Option<Harness> {
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
    .bind(unique("engine"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let engine = Engine::new(
        pools.app.clone(),
        huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring()),
    );
    Some(Harness { pools, engine, org })
}

fn document(spec: &str) -> Json {
    let yaml = format!(
        "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: test, name: Test }}\nspec:\n{spec}"
    );
    serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct RunRow {
    status: RunStatus,
    outputs: Option<Json>,
    error: Option<Json>,
}

impl Harness {
    async fn start(&self, spec: &str, inputs: Json) -> Uuid {
        self.start_in(self.org, spec, inputs).await
    }

    async fn start_in(&self, org: Uuid, spec: &str, inputs: Json) -> Uuid {
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
                    document: Some(document(spec)),
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs,
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

    /// Processes jobs until the run finishes.
    async fn finish(&self, id: Uuid) -> RunRow {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.engine.drain("test-worker", 50).await.unwrap();
            let run = self.run(id).await;
            if run.status.is_finished() {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} didn't finish: {run:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Latest attempt of each step: key → (status, output, error).
    async fn steps(&self, id: Uuid) -> Vec<(String, String, Option<Json>, Option<Json>)> {
        sqlx::query_as(
            "SELECT DISTINCT ON (step_key) step_key, status::text, output, error \
               FROM step_runs WHERE run_id = $1 ORDER BY step_key, attempt DESC",
        )
        .bind(id)
        .fetch_all(&self.pools.owner)
        .await
        .unwrap()
    }

    async fn status_of(&self, id: Uuid, key: &str) -> String {
        self.steps(id)
            .await
            .into_iter()
            .find(|s| s.0 == key)
            .map_or_else(|| "missing".into(), |s| s.1)
    }

    async fn jobs(&self, id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE run_id = $1")
            .bind(id)
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn pure_steps_run_in_one_pass() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            r#"  steps:
    - { id: greet, type: transform, output: "Hello {{ inputs.name }}" }
    - { id: remember, type: set_var, vars: { count: "{{ inputs.n * 2 }}" } }
    - { id: skipped, type: transform, if: "{{ false }}", output: never }
    - id: choose
      type: if
      condition: "{{ run.vars.count > 5 }}"
      then: [ { id: big, type: transform, output: "{{ run.vars.count }} is big" } ]
      else: [ { id: small, type: transform, output: small } ]
    - id: route
      type: switch
      value: "{{ inputs.name | lower }}"
      cases:
        - { when: bob, steps: [ { id: is_bob, type: transform, output: 1 } ] }
        - { when: ada, steps: [ { id: is_ada, type: transform, output: 2 } ] }
  outputs:
    greeting: "{{ steps.greet.output }}"
    branch: "{{ steps.choose.output }}"
    route: "{{ steps.route.output }}"
    skipped: "{{ steps.skipped.output }}"
"#,
            json!({ "name": "Ada", "n": 4 }),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({ "greeting": "Hello Ada", "branch": "8 is big", "route": 2, "skipped": null })
    );
    assert_eq!(h.status_of(id, "skipped").await, "skipped");
    assert_eq!(h.status_of(id, "small").await, "missing");
    assert_eq!(h.jobs(id).await, 0);
}

#[tokio::test]
async fn foreach_and_parallel_wait_on_scheduled_steps() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            r#"  steps:
    - id: each
      type: foreach
      items: "{{ inputs.items }}"
      as: thing
      concurrency: 2
      steps:
        - { id: pause, type: wait, duration: 10ms }
        - { id: shout, type: transform, output: "{{ thing | upper }}{{ loop.index }}" }
    - id: both
      type: parallel
      branches:
        - { id: left, steps: [ { id: l_wait, type: wait, duration: 10ms }, { id: l, type: transform, output: L } ] }
        - { id: right, steps: [ { id: r, type: transform, output: R } ] }
    - id: first
      type: parallel
      join: any
      branches:
        - { id: slow, steps: [ { id: s_wait, type: wait, duration: 1h }, { id: s, type: transform, output: slow } ] }
        - { id: fast, steps: [ { id: f, type: transform, output: fast } ] }
  outputs:
    each: "{{ steps.each.output }}"
    both: "{{ steps.both.output }}"
    first: "{{ steps.first.output }}"
"#,
            json!({ "items": ["a", "b", "c"] }),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({
            "each": ["A1", "B2", "C3"],
            "both": { "left": "L", "right": "R" },
            "first": { "fast": "fast" },
        })
    );
    // Iteration keys, and the losing branch's wait was cancelled.
    assert_eq!(h.status_of(id, "shout[2]").await, "succeeded");
    assert_eq!(h.status_of(id, "s_wait").await, "cancelled");
    assert_eq!(h.jobs(id).await, 0);
}

#[tokio::test]
async fn errors_are_caught_handled_and_reported() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            r#"  steps:
    - id: guarded
      type: try
      steps:
        - { id: boom, type: fail, message: "bad {{ inputs.what }}", code: E1 }
      catch:
        - { id: caught, type: transform, output: "caught {{ error.message }} ({{ error.code }})" }
      finally:
        - { id: tidy, type: transform, output: tidied }
    - { id: shrug, type: fail, message: ignored, continue_on_error: true }
    - { id: typo, type: transform, output: "{{ inputs.missing.deeper }}" }
    - { id: never, type: transform, output: no }
  on_error:
    - { id: report, type: transform, output: "handled {{ error.kind }}" }
"#,
            json!({ "what": "thing" }),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    let error = run.error.unwrap();
    assert_eq!(error["kind"], "expression");
    assert!(run.outputs.is_none());
    let steps = h.steps(id).await;
    let output = |key: &str| steps.iter().find(|s| s.0 == key).and_then(|s| s.2.clone());
    assert_eq!(output("guarded"), Some(json!("caught bad thing (E1)")));
    assert_eq!(output("tidy"), Some(json!("tidied")));
    assert_eq!(output("report"), Some(json!("handled expression")));
    assert_eq!(h.status_of(id, "shrug").await, "failed");
    assert_eq!(h.status_of(id, "never").await, "missing");
}

#[tokio::test]
async fn a_step_held_by_a_dead_worker_is_picked_up_again() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            r#"  steps:
    - { id: pause, type: wait, duration: 400ms }
    - { id: after, type: transform, output: done }
  outputs: { result: "{{ steps.after.output }}" }
"#,
            json!({}),
        )
        .await;
    // Advance until the wait is scheduled, then "crash" while holding it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.status_of(id, "pause").await != "waiting" {
        h.engine.drain("test-worker", 5).await.unwrap();
        assert!(Instant::now() < deadline);
    }
    // Claimed (by a worker that then dies) before it's due, so no other
    // test's worker can take it first.
    let held = sqlx::query(
        "UPDATE jobs SET locked_by = 'crashed', locked_until = now() + interval '1500 milliseconds' \
          WHERE run_id = $1 AND kind = 'step'",
    )
    .bind(id)
    .execute(&h.pools.owner)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(held, 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.engine.drain("test-worker", 50).await.unwrap();
    assert_eq!(
        h.run(id).await.status,
        RunStatus::Running,
        "due, but still leased"
    );

    // The lease runs out and another worker finishes the run.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(run.outputs.unwrap(), json!({ "result": "done" }));
}

#[tokio::test]
async fn cancelling_stops_the_run() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            "  steps:\n    - { id: nap, type: wait, duration: 1h }\n",
            json!({}),
        )
        .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.status_of(id, "nap").await != "waiting" {
        h.engine.drain("test-worker", 5).await.unwrap();
        assert!(Instant::now() < deadline);
    }
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    assert!(h.engine.cancel_run(&mut tx, id).await.unwrap());
    tx.commit().await.unwrap();
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Cancelled);
    assert_eq!(h.status_of(id, "nap").await, "cancelled");
    assert_eq!(h.jobs(id).await, 0);
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    assert!(
        !h.engine.cancel_run(&mut tx, id).await.unwrap(),
        "already finished"
    );
}

#[tokio::test]
async fn runs_time_out() {
    let Some(h) = harness().await else { return };
    let id = h
        .start(
            "  settings: { timeout: 1s }\n  steps:\n    - { id: nap, type: wait, duration: 1h }\n",
            json!({}),
        )
        .await;
    h.engine.drain("test-worker", 10).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.error.unwrap()["kind"], "timeout");
    assert_eq!(h.status_of(id, "nap").await, "cancelled");
}

#[tokio::test]
async fn decisions_are_journaled_once() {
    let Some(h) = harness().await else { return };
    // The id is generated once even though the run is walked several times.
    let id = h
        .start(
            r#"  steps:
    - { id: made, type: transform, output: "{{ uuid() }}" }
    - { id: pause, type: wait, duration: 5ms }
    - { id: again, type: transform, output: "{{ steps.made.output }}" }
  outputs: { same: "{{ steps.made.output == steps.again.output }}" }
"#,
            json!({}),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.outputs.unwrap(), json!({ "same": true }));
}

/// Publishes a workflow in the harness org.
async fn publish(h: &Harness, key: &str, spec: &str) {
    publish_in(h, h.org, key, spec).await;
}

/// Publishes a workflow in `org`.
async fn publish_in(h: &Harness, org: Uuid, key: &str, spec: &str) {
    let yaml = format!(
        "apiVersion: usehuxley.com/v1\nkind: Workflow\nmetadata: {{ key: {key}, name: {key} }}\nspec:\n{spec}"
    );
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&yaml).unwrap()).unwrap();
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    let expr = huxley_expr::Engine::new();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let created = huxley_workflows::create(&mut tx, &ctx, &expr, org, document)
        .await
        .unwrap();
    huxley_workflows::publish(&mut tx, &ctx, &expr, created.summary.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn subworkflows_return_outputs_and_failures() {
    let Some(h) = harness().await else { return };
    publish(
        &h,
        "double",
        r#"  inputs: { type: object, required: [n], properties: { n: { type: integer } } }
  steps:
    - { id: pause, type: wait, duration: 5ms }
    - { id: twice, type: transform, output: "{{ inputs.n * 2 }}" }
  outputs: { result: "{{ steps.twice.output }}" }
"#,
    )
    .await;
    publish(
        &h,
        "explode",
        "  steps:\n    - { id: boom, type: fail, message: child broke }\n",
    )
    .await;
    let id = h
        .start(
            r#"  steps:
    - { id: call, type: subworkflow, workflow: double, with: { n: 21 } }
    - { id: fire, type: subworkflow, workflow: double, with: { n: 1 }, wait: false }
    - { id: bad, type: subworkflow, workflow: explode, continue_on_error: true }
    - { id: missing, type: subworkflow, workflow: nowhere, continue_on_error: true }
    - { id: wrong, type: subworkflow, workflow: double, with: { n: "x" }, continue_on_error: true }
  outputs:
    result: "{{ steps.call.output.result }}"
    fired: "{{ steps.fire.output.run_id is defined }}"
    bad: "{{ steps.bad.output.message }}"
    missing: "{{ steps.missing.output.kind }}"
    wrong: "{{ steps.wrong.output.kind }}"
"#,
            json!({}),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({
            "result": 42,
            "fired": true,
            "bad": "child broke",
            "missing": "definition",
            "wrong": "inputs",
        })
    );
    let children: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT status::text, parent_step_key FROM runs WHERE parent_run_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(children.len(), 3, "{children:?}");
    assert!(
        children
            .iter()
            .any(|c| c.1.as_deref() == Some("bad") && c.0 == "failed")
    );
}

#[tokio::test]
async fn cancelling_a_parent_cancels_its_children() {
    let Some(h) = harness().await else { return };
    publish(
        &h,
        "sleepy",
        "  steps:\n    - { id: nap, type: wait, duration: 1h }\n",
    )
    .await;
    let id = h
        .start(
            "  steps:\n    - { id: call, type: subworkflow, workflow: sleepy }\n",
            json!({}),
        )
        .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let child = loop {
        h.engine.drain("test-worker", 10).await.unwrap();
        let child: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM runs WHERE parent_run_id = $1 AND status = 'running'",
        )
        .bind(id)
        .fetch_optional(&h.pools.owner)
        .await
        .unwrap();
        if let Some(child) = child {
            break child;
        }
        assert!(Instant::now() < deadline, "child never started");
    };
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    h.engine.cancel_run(&mut tx, id).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(h.finish(id).await.status, RunStatus::Cancelled);
    assert_eq!(h.finish(child).await.status, RunStatus::Cancelled);
}

/// Adds a client org under `parent`.
async fn client_org(h: &Harness, parent: Uuid) -> Uuid {
    let org = Uuid::now_v7();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path) \
         VALUES ($1, $2, 'client', $3, '')",
    )
    .bind(org)
    .bind(parent)
    .bind(unique("client"))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    org
}

#[tokio::test]
async fn subworkflows_are_found_on_the_nearest_ancestor() {
    let Some(h) = harness().await else { return };
    // provider (h.org) → client → site. "shared" is published on the
    // provider and overridden on the client; "only-top" is only on the
    // provider.
    let client = client_org(&h, h.org).await;
    let site = client_org(&h, client).await;
    publish(
        &h,
        "shared",
        "  steps:\n    - { id: x, type: transform, output: 1 }\n  outputs: { from: provider }\n",
    )
    .await;
    publish_in(
        &h,
        client,
        "shared",
        "  steps:\n    - { id: x, type: transform, output: 1 }\n  outputs: { from: client }\n",
    )
    .await;
    publish(
        &h,
        "only-top",
        "  steps:\n    - { id: x, type: transform, output: 1 }\n  outputs: { from: provider }\n",
    )
    .await;
    let spec = r#"  steps:
    - { id: shared, type: subworkflow, workflow: shared }
    - { id: top, type: subworkflow, workflow: only-top }
  outputs:
    shared: "{{ steps.shared.output.from }}"
    top: "{{ steps.top.output.from }}"
"#;
    let id = h.start_in(site, spec, json!({})).await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");
    assert_eq!(
        run.outputs.unwrap(),
        json!({ "shared": "client", "top": "provider" })
    );
    // Children run in the caller's org, not where the workflow lives.
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT org_id FROM runs WHERE parent_run_id = $1")
        .bind(id)
        .fetch_all(&h.pools.owner)
        .await
        .unwrap();
    assert_eq!(orgs, vec![site, site]);

    // A workflow published only below the caller isn't visible to it.
    let below = h
        .start(
            "  steps:\n    - { id: call, type: subworkflow, workflow: shared, continue_on_error: true }\n  outputs: { from: \"{{ steps.call.output.from }}\" }\n",
            json!({}),
        )
        .await;
    assert_eq!(
        h.finish(below).await.outputs.unwrap(),
        json!({ "from": "provider" })
    );
}

#[tokio::test]
async fn subworkflows_stop_at_the_nesting_limit() {
    let Some(h) = harness().await else { return };
    // A workflow that calls itself forever.
    publish(
        &h,
        "forever",
        "  steps:\n    - { id: again, type: subworkflow, workflow: forever }\n",
    )
    .await;
    let id = h
        .start(
            "  steps:\n    - { id: call, type: subworkflow, workflow: forever }\n",
            json!({}),
        )
        .await;
    let run = h.finish(id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    // The whole tree, root included, is at most 8 runs deep.
    let depth: i64 = sqlx::query_scalar(
        "WITH RECURSIVE tree AS ( \
             SELECT id, 1 AS depth FROM runs WHERE id = $1 \
             UNION ALL \
             SELECT r.id, t.depth + 1 FROM runs r JOIN tree t ON r.parent_run_id = t.id) \
         SELECT max(depth)::int8 FROM tree",
    )
    .bind(id)
    .fetch_one(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(depth, 8);
    // The deepest run failed on the limit; the error reaches the root.
    let error = run.error.unwrap().to_string();
    assert!(error.contains("nest at most 8"), "{error}");
}

#[tokio::test]
async fn failed_runs_resume_from_the_failed_step() {
    let Some(h) = harness().await else { return };
    // Fails until the org variable `resume_flag` exists.
    let id = h
        .start(
            r#"  steps:
    - { id: first, type: transform, output: "{{ uuid() }}" }
    - id: loop
      type: foreach
      items: "{{ [1, 2, 3] }}"
      steps:
        - id: check
          type: transform
          output: "{{ vars.resume_flag * item }}"
  outputs:
    first: "{{ steps.first.output }}"
    loop: "{{ steps.loop.output }}"
  on_error:
    - { id: report, type: transform, output: reported }
"#,
            json!({}),
        )
        .await;
    let failed = h.finish(id).await;
    assert_eq!(failed.status, RunStatus::Failed, "{failed:?}");
    let first_output: Json =
        sqlx::query_scalar("SELECT output FROM step_runs WHERE run_id = $1 AND step_key = 'first'")
            .bind(id)
            .fetch_one(&h.pools.owner)
            .await
            .unwrap();

    // Fix the cause, then resume.
    let ctx = huxley_core::RequestContext::system(huxley_core::Via::System);
    let crypto =
        huxley_crypto::Crypto::new(h.pools.app.clone(), huxley_crypto::testing::key_ring());
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    let input = huxley_variables::VariableInput {
        kind: huxley_variables::VariableType::Number,
        value: json!(10),
        description: None,
        locked: false,
    };
    huxley_variables::set(&mut tx, &ctx, &crypto, h.org, "resume_flag", &input)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    assert!(h.engine.resume_run(&mut tx, id).await.unwrap());
    tx.commit().await.unwrap();

    let resumed = h.finish(id).await;
    assert_eq!(resumed.status, RunStatus::Succeeded, "{resumed:?}");
    let outputs = resumed.outputs.unwrap();
    // Steps that had succeeded weren't run again.
    assert_eq!(outputs["first"], first_output);
    assert_eq!(outputs["loop"], json!([10, 20, 30]));
    // The failed attempt is kept, superseded; the retry is attempt 2.
    let attempts: Vec<(i32, String, bool)> = sqlx::query_as(
        "SELECT attempt, status::text, superseded FROM step_runs \
          WHERE run_id = $1 AND step_key = 'check[0]' ORDER BY attempt",
    )
    .bind(id)
    .fetch_all(&h.pools.owner)
    .await
    .unwrap();
    assert_eq!(
        attempts,
        vec![(1, "failed".into(), true), (2, "succeeded".into(), false)]
    );
    // Only failed runs resume.
    let mut tx = begin_scoped(&h.pools.app, &OrgScope::System).await.unwrap();
    assert!(!h.engine.resume_run(&mut tx, id).await.unwrap());
}
