//! The job queue (ADR-002): a table claimed with `FOR UPDATE SKIP LOCKED`.
//!
//! Jobs are enqueued in the caller's transaction, so they exist exactly when
//! the change that needs them commits. A claimed job is leased until
//! `locked_until`; the worker extends the lease while it works, and if the
//! worker dies the lease runs out and another worker claims the job.

use std::time::Duration;

use serde_json::Value as Json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

/// The channel workers listen on for new jobs.
pub const CHANNEL: &str = "huxley_jobs";

/// Kinds of job.
pub mod kind {
    /// Walk a run and make progress.
    pub const ADVANCE: &str = "advance";
    /// Execute one step.
    pub const STEP: &str = "step";
    /// A run's timeout: advance it so it fails.
    pub const DEADLINE: &str = "deadline";
}

/// Infrastructure failures before a job is given up on (dead-lettered).
pub const MAX_JOB_ATTEMPTS: i32 = 10;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub org_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub step_key: Option<String>,
    pub payload: Json,
    pub attempts: i32,
}

/// Queues an advance of `run_id`, unless one is already queued.
pub async fn enqueue_advance(
    conn: &mut PgConnection,
    org_id: Uuid,
    run_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, kind, org_id, run_id) VALUES ($1, 'advance', $2, $3) \
         ON CONFLICT (run_id) WHERE kind = 'advance' AND locked_by IS NULL AND NOT dead \
         DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(run_id)
    .execute(&mut *conn)
    .await?;
    notify(conn).await
}

/// Queues an advance of `run_id` for later (a run waiting for a slot).
pub async fn enqueue_advance_at(
    conn: &mut PgConnection,
    org_id: Uuid,
    run_id: Uuid,
    at: OffsetDateTime,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, kind, org_id, run_id, run_at) VALUES ($1, 'advance', $2, $3, $4) \
         ON CONFLICT (run_id) WHERE kind = 'advance' AND locked_by IS NULL AND NOT dead \
         DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(run_id)
    .bind(at)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Queues execution of a step, at `run_at` (now if `None`), unless it is
/// already queued.
pub async fn enqueue_step(
    conn: &mut PgConnection,
    org_id: Uuid,
    run_id: Uuid,
    step_key: &str,
    run_at: Option<OffsetDateTime>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, kind, org_id, run_id, step_key, run_at) \
         VALUES ($1, 'step', $2, $3, $4, COALESCE($5, now())) \
         ON CONFLICT (run_id, step_key) WHERE kind = 'step' AND NOT dead DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(run_id)
    .bind(step_key)
    .bind(run_at)
    .execute(&mut *conn)
    .await?;
    if run_at.is_none_or(|at| at <= OffsetDateTime::now_utc()) {
        notify(conn).await?;
    }
    Ok(())
}

/// Queues a run's deadline.
pub async fn enqueue_deadline(
    conn: &mut PgConnection,
    org_id: Uuid,
    run_id: Uuid,
    at: OffsetDateTime,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, kind, org_id, run_id, run_at) VALUES ($1, 'deadline', $2, $3, $4)",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(run_id)
    .bind(at)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Wakes idle workers once the transaction commits.
pub async fn notify(conn: &mut PgConnection) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(CHANNEL)
        .execute(conn)
        .await
        .map(|_| ())
}

/// Claims up to `limit` due jobs for `worker`, leased for `lease`.
pub async fn claim(
    conn: &mut PgConnection,
    worker: &str,
    limit: i64,
    lease: Duration,
) -> sqlx::Result<Vec<Job>> {
    // A CTE, not `WHERE id IN (SELECT … LIMIT n)`: the planner may run such
    // a subquery more than once and claim more than `limit` rows.
    sqlx::query_as(
        "WITH next AS ( \
                SELECT id FROM jobs \
                 WHERE NOT dead AND run_at <= now() \
                   AND (locked_until IS NULL OR locked_until < now()) \
                 ORDER BY priority DESC, run_at \
                 LIMIT $2 FOR UPDATE SKIP LOCKED) \
         UPDATE jobs SET locked_by = $1, \
                locked_until = now() + make_interval(secs => $3), \
                attempts = jobs.attempts + 1 \
           FROM next WHERE jobs.id = next.id \
         RETURNING jobs.id, jobs.kind, jobs.org_id, jobs.run_id, jobs.step_key, jobs.payload, \
                   jobs.attempts",
    )
    .bind(worker)
    .bind(limit)
    .bind(lease.as_secs_f64())
    .fetch_all(conn)
    .await
}

/// Extends the lease on a job this worker holds. Returns false if it no
/// longer holds it (the lease ran out and someone else claimed it).
pub async fn heartbeat(
    conn: &mut PgConnection,
    job: Uuid,
    worker: &str,
    lease: Duration,
) -> sqlx::Result<bool> {
    let updated = sqlx::query(
        "UPDATE jobs SET locked_until = now() + make_interval(secs => $3) \
          WHERE id = $1 AND locked_by = $2",
    )
    .bind(job)
    .bind(worker)
    .bind(lease.as_secs_f64())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Removes a finished job (only if this worker still holds it).
pub async fn complete(conn: &mut PgConnection, job: Uuid, worker: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = $1 AND locked_by = $2")
        .bind(job)
        .bind(worker)
        .execute(conn)
        .await
        .map(|_| ())
}

/// Puts a job back after an infrastructure failure, with exponential
/// backoff, or dead-letters it once it has used its attempts. Returns true
/// if it was dead-lettered.
pub async fn fail(conn: &mut PgConnection, job: &Job, error: &str) -> sqlx::Result<bool> {
    let dead = job.attempts >= MAX_JOB_ATTEMPTS;
    let backoff = 2f64.powi(job.attempts.clamp(1, 10)).min(600.0);
    sqlx::query(
        "UPDATE jobs SET locked_by = NULL, locked_until = NULL, last_error = $2, dead = $3, \
                run_at = now() + make_interval(secs => $4) \
          WHERE id = $1",
    )
    .bind(job.id)
    .bind(error.chars().take(2000).collect::<String>())
    .bind(dead)
    .bind(backoff)
    .execute(conn)
    .await?;
    Ok(dead)
}

/// Re-queues a step job this worker holds to run again at `run_at` (a
/// retry the step's policy asked for; not an infrastructure failure).
pub async fn reschedule(
    conn: &mut PgConnection,
    job: Uuid,
    worker: &str,
    run_at: OffsetDateTime,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE jobs SET locked_by = NULL, locked_until = NULL, attempts = 0, run_at = $3 \
          WHERE id = $1 AND locked_by = $2",
    )
    .bind(job)
    .bind(worker)
    .bind(run_at)
    .execute(conn)
    .await
    .map(|_| ())
}

/// Deletes every queued job for a run (when it ends or is cancelled),
/// except `keep` (the job doing the deleting).
pub async fn delete_for_run(
    conn: &mut PgConnection,
    run_id: Uuid,
    keep: Option<Uuid>,
) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM jobs WHERE run_id = $1 AND ($2::uuid IS NULL OR id <> $2)")
            .bind(run_id)
            .bind(keep)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}
