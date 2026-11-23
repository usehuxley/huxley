//! Chaos test: real `huxley serve --roles worker` processes are killed
//! (SIGKILL) and restarted while runs execute. Every run must still finish,
//! with every step journaled as succeeded exactly once (ADR-003, ADR-021).
//!
//! Uses its own database, created and dropped here, so other tests' jobs
//! and keys don't interfere. Needs `HUXLEY_TEST_DATABASE_URL` with a role
//! that may create databases.

mod common;

use std::process::Child;
use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root};
use huxley_db::{DatabaseConfig, OrgScope, begin_scoped};
use secrecy::SecretString;
use serde_json::{Value, json};
use uuid::Uuid;

use common::{Database, worker};

const WORKERS: usize = 3;
const RUNS: usize = 8;

const DOCUMENT: &str = r#"
apiVersion: usehuxley.com/v1
kind: Workflow
metadata: { key: chaos, name: Chaos }
spec:
  steps:
    - id: each
      type: foreach
      items: "{{ range(16) | list }}"
      concurrency: 4
      steps:
        - { id: pause, type: wait, duration: 500ms }
        - { id: double, type: transform, output: "{{ item * 2 }}" }
    - id: both
      type: parallel
      branches:
        - { id: a, steps: [ { id: wait_a, type: wait, duration: 600ms }, { id: done_a, type: transform, output: A } ] }
        - { id: b, steps: [ { id: wait_b, type: wait, duration: 200ms }, { id: done_b, type: transform, output: B } ] }
  outputs:
    each: "{{ steps.each.output }}"
    both: "{{ steps.both.output }}"
"#;

async fn setup(db: &Database) -> (TestPools, Vec<Uuid>) {
    let config = DatabaseConfig {
        url: SecretString::from(db.url.clone()),
        max_connections: 4,
        acquire_timeout_secs: 10,
        statement_timeout_ms: 10_000,
    };
    let owner = huxley_db::connect_owner(&config).unwrap();
    huxley_db::migrate(&owner).await.unwrap();
    let pools = TestPools {
        owner,
        app: huxley_db::connect(&config).unwrap(),
    };
    let root = ensure_root(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let expr = huxley_expr::Engine::new();
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto);
    let document = serde_json::to_value(huxley_hwf::parse_yaml(DOCUMENT).unwrap()).unwrap();

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let workflow = huxley_workflows::create(&mut tx, &ctx, &expr, root, document)
        .await
        .unwrap();
    let id = workflow.summary.id;
    huxley_workflows::publish(&mut tx, &ctx, &expr, id, None)
        .await
        .unwrap();
    let mut runs = Vec::new();
    for _ in 0..RUNS {
        runs.push(
            engine
                .start_run(
                    &mut tx,
                    huxley_engine::StartRun {
                        org_id: root,
                        workflow_id: Some(id),
                        version: Some(1),
                        document: None,
                        trigger_type: "manual".into(),
                        trigger: json!({}),
                        inputs: json!({}),
                        actor_id: None,
                    },
                )
                .await
                .unwrap(),
        );
    }
    tx.commit().await.unwrap();
    (pools, runs)
}

#[tokio::test]
async fn killed_workers_lose_no_steps() {
    let Some(db) = Database::create("chaos").await else {
        return;
    };
    let (pools, runs) = setup(&db).await;

    let mut workers: Vec<Child> = (0..WORKERS).map(|_| worker(&db.url, 4)).collect();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut kills = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let unfinished: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM runs WHERE status NOT IN ('succeeded', 'failed', 'cancelled')",
        )
        .fetch_one(&pools.owner)
        .await
        .unwrap();
        if unfinished == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{unfinished} runs never finished after {kills} kills"
        );
        // Kill one worker mid-flight and start a replacement, round robin.
        if kills < 30 {
            let victim = kills % WORKERS;
            let _ = workers[victim].kill();
            let _ = workers[victim].wait();
            workers[victim] = worker(&db.url, 4);
            kills += 1;
        }
    }
    for mut w in workers {
        let _ = w.kill();
        let _ = w.wait();
    }

    let finished: Vec<(Uuid, String, Option<Value>)> =
        sqlx::query_as("SELECT id, status::text, outputs FROM runs ORDER BY id")
            .fetch_all(&pools.owner)
            .await
            .unwrap();
    assert_eq!(finished.len(), RUNS);
    let expected = json!({
        "each": [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30],
        "both": { "a": "A", "b": "B" },
    });
    for (id, status, outputs) in &finished {
        assert_eq!(status, "succeeded", "run {id}");
        assert_eq!(outputs.as_ref(), Some(&expected), "run {id}");
    }
    // Every step key of every run succeeded exactly once.
    let per_run_steps = 1 + 16 * 2 + 1 + 4;
    let doubles: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT run_id, step_key, count(*) FILTER (WHERE status = 'succeeded') AS ok \
           FROM step_runs GROUP BY run_id, step_key \
          HAVING count(*) FILTER (WHERE status = 'succeeded') <> 1",
    )
    .fetch_all(&pools.owner)
    .await
    .unwrap();
    assert_eq!(doubles, vec![], "steps not succeeded exactly once");
    let keys: i64 = sqlx::query_scalar("SELECT count(DISTINCT (run_id, step_key)) FROM step_runs")
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(keys, i64::try_from(RUNS * per_run_steps).unwrap());
    assert!(
        kills >= 5,
        "only {kills} kills happened before the runs finished"
    );
    let _ = runs;

    pools.owner.close().await;
    pools.app.close().await;
    db.drop().await;
}
