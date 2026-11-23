//! The job queue against a real PostgreSQL.

use std::time::Duration;

use huxley_db::testing::pools;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::queue;
use uuid::Uuid;

/// Regression: `UPDATE … WHERE id IN (SELECT … LIMIT n FOR UPDATE SKIP
/// LOCKED)` can claim more than n rows; claimed-but-unreturned jobs were
/// then stuck until their lease ran out.
#[tokio::test]
async fn claim_takes_exactly_the_limit() {
    let Some(pools) = pools().await else { return };
    let worker = format!("claim-test-{}", Uuid::now_v7().simple());
    let runs: Vec<Uuid> = (0..30).map(|_| Uuid::now_v7()).collect();
    // Advances of runs that don't exist: harmless if another test's worker
    // processes one. High priority, so they're first in line.
    for run in &runs {
        sqlx::query(
            "INSERT INTO jobs (id, kind, run_id, priority) VALUES ($1, 'advance', $2, 100)",
        )
        .bind(Uuid::now_v7())
        .bind(run)
        .execute(&pools.owner)
        .await
        .unwrap();
    }
    for round in 1..=5 {
        let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
        let claimed = queue::claim(&mut tx, &worker, 1, Duration::from_secs(60))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(claimed.len() <= 1);
        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE locked_by = $1")
            .bind(&worker)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
        assert!(
            held <= round,
            "round {round}: {held} jobs held after claiming one at a time"
        );
    }
    sqlx::query("DELETE FROM jobs WHERE run_id = ANY($1)")
        .bind(&runs)
        .execute(&pools.owner)
        .await
        .unwrap();
}

#[tokio::test]
async fn leases_heartbeats_and_dead_letters() {
    let Some(pools) = pools().await else { return };
    let worker = format!("lease-test-{}", Uuid::now_v7().simple());
    // A job this worker already holds, on its last attempt. (Claiming it
    // here would race other test processes' workers.)
    let job = queue::Job {
        id: Uuid::now_v7(),
        kind: "advance".into(),
        org_id: None,
        run_id: Some(Uuid::now_v7()),
        step_key: None,
        payload: serde_json::json!({}),
        attempts: queue::MAX_JOB_ATTEMPTS,
    };
    sqlx::query(
        "INSERT INTO jobs (id, kind, run_id, attempts, locked_by, locked_until) \
         VALUES ($1, 'advance', $2, $3, $4, now() + interval '1 minute')",
    )
    .bind(job.id)
    .bind(job.run_id)
    .bind(job.attempts)
    .bind(&worker)
    .execute(&pools.owner)
    .await
    .unwrap();

    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    assert!(
        queue::heartbeat(&mut tx, job.id, &worker, Duration::from_secs(60))
            .await
            .unwrap()
    );
    assert!(
        !queue::heartbeat(&mut tx, job.id, "someone-else", Duration::from_secs(60))
            .await
            .unwrap()
    );
    // Out of attempts: dead-lettered.
    assert!(queue::fail(&mut tx, &job, "boom").await.unwrap());
    tx.commit().await.unwrap();
    let (dead, error): (bool, Option<String>) =
        sqlx::query_as("SELECT dead, last_error FROM jobs WHERE id = $1")
            .bind(job.id)
            .fetch_one(&pools.owner)
            .await
            .unwrap();
    assert!(dead);
    assert_eq!(error.as_deref(), Some("boom"));
    sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(job.id)
        .execute(&pools.owner)
        .await
        .unwrap();
}
