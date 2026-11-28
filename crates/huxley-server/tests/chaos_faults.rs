//! More chaos (plan §4, reliability), beside `chaos.rs` (workers killed and
//! restarted while runs of internal steps execute): a worker killed in the
//! middle of a call to an outside service, and every database connection dropped while runs are
//! in flight. Neither may lose a run: the work is picked up again (leases
//! run out; pools and LISTEN reconnect) and every run finishes, with
//! nothing dead-lettered.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root};
use huxley_db::{DatabaseConfig, OrgScope, begin_scoped};
use secrecy::SecretString;
use serde_json::json;
use uuid::Uuid;

use common::{Database, worker_with};

/// Runs started in each test.
const KILL_RUNS: usize = 6;
const DB_RUNS: usize = 10;

/// Workers may call the slow endpoint on localhost.
const EGRESS: (&str, &str) = ("HUXLEY_ENGINE__EGRESS__PRIVATE_HOSTS", "[localhost]");

struct Instance {
    db: Database,
    pools: TestPools,
    engine: huxley_engine::Engine,
    root: Uuid,
}

async fn instance(prefix: &str) -> Option<Instance> {
    let db = Database::create(prefix).await?;
    let config = DatabaseConfig {
        url: SecretString::from(db.url.clone()),
        max_connections: 8,
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
    let crypto = huxley_crypto::Crypto::new(pools.app.clone(), huxley_crypto::testing::key_ring());
    let engine = huxley_engine::Engine::new(pools.app.clone(), crypto);
    Some(Instance {
        db,
        pools,
        engine,
        root,
    })
}

impl Instance {
    async fn publish(&self, yaml: &str) -> Uuid {
        let ctx = RequestContext::system(Via::System);
        let expr = huxley_expr::Engine::new();
        let document = serde_json::to_value(huxley_hwf::parse_yaml(yaml).unwrap()).unwrap();
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        let workflow = huxley_workflows::create(&mut tx, &ctx, &expr, self.root, document)
            .await
            .unwrap();
        huxley_workflows::publish(&mut tx, &ctx, &expr, workflow.summary.id, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        workflow.summary.id
    }

    async fn start(&self, workflow: Uuid, runs: usize) {
        let mut tx = begin_scoped(&self.pools.app, &OrgScope::System)
            .await
            .unwrap();
        for _ in 0..runs {
            self.engine
                .start_run(
                    &mut tx,
                    huxley_engine::StartRun {
                        org_id: self.root,
                        workflow_id: Some(workflow),
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
    }

    /// Waits for every run to finish; returns `(succeeded, other)`.
    async fn finished(&self, within: Duration) -> (i64, i64) {
        let started = Instant::now();
        loop {
            let counts: Option<(i64, i64, i64)> = sqlx::query_as(
                "SELECT count(*) FILTER (WHERE finished_at IS NULL), \
                        count(*) FILTER (WHERE status = 'succeeded'), \
                        count(*) FILTER (WHERE finished_at IS NOT NULL AND status <> 'succeeded') FROM runs",
            )
            .fetch_one(&self.pools.owner)
            .await
            .ok();
            if let Some((0, ok, other)) = counts {
                return (ok, other);
            }
            assert!(
                started.elapsed() < within,
                "runs didn't finish in {within:?}: {counts:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn dead_jobs(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE dead")
            .fetch_one(&self.pools.owner)
            .await
            .unwrap()
    }
}

/// An endpoint that takes `delay` to answer, counting calls.
async fn slow_endpoint(delay: Duration) -> (u16, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let app = axum::Router::new().route(
        "/slow",
        axum::routing::get(move || {
            let calls = counted.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                axum::Json(json!({ "ok": true }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, calls)
}

fn stop(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn a_worker_killed_mid_step_loses_no_run() {
    let Some(h) = instance("chaos_kill").await else {
        return;
    };
    let (port, calls) = slow_endpoint(Duration::from_secs(3)).await;
    let workflow = h
        .publish(&format!(
            r#"
apiVersion: usehuxley.io/v1
kind: Workflow
metadata: {{ key: chaos-kill, name: Chaos kill }}
spec:
  steps:
    - id: call
      type: http.request
      with: {{ url: "http://localhost:{port}/slow" }}
    - id: after
      type: transform
      output: "{{{{ steps.call.output.body.ok }}}}"
  outputs:
    ok: "{{{{ steps.after.output }}}}"
"#
        ))
        .await;
    let first = worker_with(&h.db.url, 8, &[EGRESS]);
    h.start(workflow, KILL_RUNS).await;

    // Kill it (SIGKILL: no shutdown) while its calls are in flight.
    let deadline = Instant::now() + Duration::from_secs(30);
    while calls.load(Ordering::SeqCst) < KILL_RUNS {
        assert!(Instant::now() < deadline, "the worker never made its calls");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop(first);
    let in_flight = calls.load(Ordering::SeqCst);

    // A new worker takes the steps over once their leases run out.
    let second = worker_with(&h.db.url, 8, &[EGRESS]);
    let (ok, other) = h.finished(Duration::from_secs(60)).await;
    stop(second);
    assert_eq!(
        (ok, other),
        (i64::try_from(KILL_RUNS).unwrap(), 0),
        "every run succeeded"
    );
    // At least once: the interrupted calls were made again.
    assert!(
        calls.load(Ordering::SeqCst) >= in_flight + KILL_RUNS,
        "the calls were retried"
    );
    let outputs: Vec<serde_json::Value> = sqlx::query_scalar("SELECT outputs FROM runs")
        .fetch_all(&h.pools.owner)
        .await
        .unwrap();
    assert!(outputs.iter().all(|o| o["ok"] == true), "{outputs:?}");
    assert_eq!(h.dead_jobs().await, 0);
    h.db.drop().await;
}

#[tokio::test]
async fn workers_survive_losing_their_database_connections() {
    let Some(h) = instance("chaos_db").await else {
        return;
    };
    let workflow = h
        .publish(
            r#"
apiVersion: usehuxley.io/v1
kind: Workflow
metadata: { key: chaos-db, name: Chaos database }
spec:
  steps:
    - id: each
      type: foreach
      items: "{{ range(5) | list }}"
      steps:
        - { id: pause, type: wait, duration: 300ms }
        - { id: square, type: transform, output: "{{ item * item }}" }
"#,
        )
        .await;
    let mut worker = worker_with(&h.db.url, 8, &[]);
    h.start(workflow, DB_RUNS).await;

    // Drop every other connection to the database, several times, while
    // the runs are in flight (the worker's pool and its LISTEN connection).
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let killed: i64 = sqlx::query_scalar(
            "SELECT count(*) FILTER (WHERE pg_terminate_backend(pid)) FROM pg_stat_activity \
              WHERE datname = current_database() AND pid <> pg_backend_pid()",
        )
        .fetch_one(&h.pools.owner)
        .await
        .unwrap();
        assert!(killed > 0, "the worker had connections to drop");
    }

    let (ok, other) = h.finished(Duration::from_secs(90)).await;
    assert!(
        worker.try_wait().unwrap().is_none(),
        "the worker kept running"
    );
    stop(worker);
    assert_eq!(
        (ok, other),
        (i64::try_from(DB_RUNS).unwrap(), 0),
        "every run succeeded"
    );
    assert_eq!(h.dead_jobs().await, 0);
    h.db.drop().await;
}
