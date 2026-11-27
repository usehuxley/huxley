//! Load test: one worker process, many runs, measured steps per second.
//! Ignored by default (it's a measurement, not a check). Run with
//!
//! ```sh
//! HUXLEY_TEST_DATABASE_URL=… cargo test --release -p huxley-server --test load -- --ignored --nocapture
//! ```
//!
//! Results are recorded in docs/performance.md.

mod common;

use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root};
use huxley_db::{DatabaseConfig, OrgScope, begin_scoped};
use secrecy::SecretString;
use serde_json::json;

use common::{Database, worker};

const ITEMS: usize = 50;
const CONCURRENCY: usize = 16;

/// Runs to start (`BENCH_RUNS`, default 40) and worker processes
/// (`BENCH_WORKERS`, default 1): the reference sizes use more of both.
fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Per run: a foreach whose iterations each schedule a step job (a 1 ms
/// wait: the cheapest side-effecting step) and evaluate a transform.
fn document() -> String {
    format!(
        r#"
apiVersion: usehuxley.io/v1
kind: Workflow
metadata: {{ key: load, name: Load }}
spec:
  steps:
    - id: each
      type: foreach
      items: "{{{{ range({ITEMS}) | list }}}}"
      concurrency: 10
      steps:
        - {{ id: tick, type: wait, duration: 1ms }}
        - {{ id: work, type: transform, output: "{{{{ item * item }}}}" }}
"#
    )
}

#[tokio::test]
#[ignore = "a measurement; run on demand"]
async fn steps_per_second() {
    let Some(db) = Database::create("load").await else {
        return;
    };
    let config = DatabaseConfig {
        url: SecretString::from(db.url.clone()),
        max_connections: 16,
        acquire_timeout_secs: 10,
        statement_timeout_ms: 10_000,
        read_url: None,
        direct_url: None,
        pooled: false,
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
    let document = serde_json::to_value(huxley_hwf::parse_yaml(&document()).unwrap()).unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let workflow = huxley_workflows::create(&mut tx, &ctx, &expr, root, document)
        .await
        .unwrap();
    huxley_workflows::publish(&mut tx, &ctx, &expr, workflow.summary.id, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (runs, workers) = (setting("BENCH_RUNS", 40), setting("BENCH_WORKERS", 1));
    let mut processes: Vec<_> = (0..workers).map(|_| worker(&db.url, CONCURRENCY)).collect();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let started = Instant::now();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    for _ in 0..runs {
        engine
            .start_run(
                &mut tx,
                huxley_engine::StartRun {
                    org_id: root,
                    workflow_id: Some(workflow.summary.id),
                    version: Some(1),
                    document: None,
                    trigger_type: "manual".into(),
                    trigger: json!({}),
                    inputs: json!({}),
                    actor_id: None,
                },
            )
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    loop {
        let unfinished: i64 =
            sqlx::query_scalar("SELECT count(*) FROM runs WHERE finished_at IS NULL")
                .fetch_one(&pools.owner)
                .await
                .unwrap();
        if unfinished == 0 {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(600), "too slow");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let elapsed = started.elapsed().as_secs_f64();
    for process in &mut processes {
        let _ = process.kill();
        let _ = process.wait();
    }

    let (steps, jobs_steps): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE step_type = 'wait') FROM step_runs",
    )
    .fetch_one(&pools.owner)
    .await
    .unwrap();
    let failed: i64 = sqlx::query_scalar("SELECT count(*) FROM runs WHERE status <> 'succeeded'")
        .fetch_one(&pools.owner)
        .await
        .unwrap();
    assert_eq!(failed, 0);
    #[allow(clippy::cast_precision_loss)]
    let rate = steps as f64 / elapsed;
    println!(
        "{runs} runs, {steps} steps ({jobs_steps} via step jobs) in {elapsed:.1}s: {rate:.0} steps/s \
         with {workers} worker process(es) at concurrency {CONCURRENCY}"
    );
    pools.owner.close().await;
    pools.app.close().await;
    db.drop().await;
}
