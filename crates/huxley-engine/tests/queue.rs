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
    let run = Uuid::now_v7();
    let worker = format!("lease-test-{}", Uuid::now_v7().simple());
    sqlx::query(
        "INSERT INTO jobs (id, kind, run_id, priority, attempts) VALUES ($1, 'advance', $2, 100, 9)",
    )
    .bind(Uuid::now_v7())
    .bind(run)
    .execute(&pools.owner)
    .await
    .unwrap();
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut claimed = Vec::new();
    // Other tests' jobs may come first; look for ours.
    for _ in 0..50 {
        let jobs = queue::claim(&mut tx, &worker, 10, Duration::from_secs(60))
            .await
            .unwrap();
        if jobs.is_empty() {
            break;
        }
        claimed.extend(jobs);
        if claimed.iter().any(|j| j.run_id == Some(run)) {
            break;
        }
    }
    let job = claimed
        .iter()
        .find(|j| j.run_id == Some(run))
        .cloned()
        .expect("claimed");
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
    // The tenth failure dead-letters it.
    assert_eq!(job.attempts, 10);
    assert!(queue::fail(&mut tx, &job, "boom").await.unwrap());
    // Release anything else we grabbed.
    for other in claimed.iter().filter(|j| j.id != job.id) {
        sqlx::query("UPDATE jobs SET locked_by = NULL, locked_until = NULL WHERE id = $1")
            .bind(other.id)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
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
