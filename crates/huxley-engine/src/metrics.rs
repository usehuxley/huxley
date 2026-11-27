//! Workflow engine metrics (OpenTelemetry; ADR-091): jobs and their
//! durations, finished runs, and — from the scheduler — the job queue, the
//! audit log's sealing backlog and SIEM streams' lag. No-ops unless
//! telemetry export is on.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use huxley_db::{OrgScope, begin_scoped};
use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use tokio_util::sync::CancellationToken;

use crate::Engine;

/// Seconds: quick bookkeeping to long calls to the outside world.
const JOB_BOUNDARIES: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0,
];

fn job_duration() -> &'static Histogram<f64> {
    static H: OnceLock<Histogram<f64>> = OnceLock::new();
    H.get_or_init(|| {
        global::meter("huxley")
            .f64_histogram("huxley.job.duration")
            .with_unit("s")
            .with_description("How long workflow jobs took, by kind and outcome")
            .with_boundaries(JOB_BOUNDARIES.to_vec())
            .build()
    })
}

fn dead_jobs() -> &'static Counter<u64> {
    static C: OnceLock<Counter<u64>> = OnceLock::new();
    C.get_or_init(|| {
        global::meter("huxley")
            .u64_counter("huxley.jobs.dead_lettered")
            .with_description("Jobs given up on after repeated failures, by kind")
            .build()
    })
}

fn runs_finished() -> &'static Counter<u64> {
    static C: OnceLock<Counter<u64>> = OnceLock::new();
    C.get_or_init(|| {
        global::meter("huxley")
            .u64_counter("huxley.runs.finished")
            .with_description("Workflow runs that finished, by status")
            .build()
    })
}

/// A job a worker finished: `ok`, or `error` (it'll be retried or
/// dead-lettered).
pub(crate) fn job_done(kind: &str, outcome: &'static str, seconds: f64) {
    job_duration().record(
        seconds,
        &[
            KeyValue::new("kind", kind.to_owned()),
            KeyValue::new("outcome", outcome),
        ],
    );
}

pub(crate) fn job_dead(kind: &str) {
    dead_jobs().add(1, &[KeyValue::new("kind", kind.to_owned())]);
}

pub(crate) fn run_finished(status: &str) {
    runs_finished().add(1, &[KeyValue::new("status", status.to_owned())]);
}

/// What the scheduler last read from the database, for the gauges.
#[derive(Debug, Default)]
struct Snapshot {
    /// (kind, state) → jobs. States: due (waiting for a worker), running,
    /// scheduled (later), dead.
    jobs: BTreeMap<(String, String), i64>,
    /// Seconds the oldest due, unclaimed job has waited.
    oldest_due: f64,
    unsealed: i64,
    /// Stream name → (records behind, failures in a row).
    streams: BTreeMap<String, (i64, i64)>,
}

type JobCount = (String, String, i64);

async fn read(engine: &Engine) -> crate::EngineResult<Snapshot> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let jobs: Vec<JobCount> = sqlx::query_as(
        "SELECT kind, CASE WHEN dead THEN 'dead' \
                           WHEN locked_until > now() THEN 'running' \
                           WHEN run_at <= now() THEN 'due' ELSE 'scheduled' END, count(*) \
           FROM jobs GROUP BY 1, 2",
    )
    .fetch_all(&mut *tx)
    .await?;
    let oldest_due: f64 = sqlx::query_scalar(
        "SELECT coalesce(extract(epoch FROM now() - min(run_at)), 0)::float8 FROM jobs \
          WHERE NOT dead AND run_at <= now() AND (locked_until IS NULL OR locked_until < now())",
    )
    .fetch_one(&mut *tx)
    .await?;
    let unsealed: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE seq IS NULL")
        .fetch_one(&mut *tx)
        .await?;
    let streams: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT name, greatest((SELECT seq FROM audit_chain_head WHERE id = 1) - last_seq, 0), failures::int8 \
           FROM audit_streams WHERE enabled",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    // Every kind and state, zero when there are none, so the series exist
    // for dashboards and alerts while the queue is empty.
    let mut counts: BTreeMap<(String, String), i64> = crate::queue::kind::ALL
        .iter()
        .flat_map(|k| {
            ["due", "running", "scheduled", "dead"]
                .into_iter()
                .map(move |s| (((*k).to_owned(), s.to_owned()), 0))
        })
        .collect();
    counts.extend(jobs.into_iter().map(|(k, s, n)| ((k, s), n)));
    Ok(Snapshot {
        jobs: counts,
        oldest_due,
        unsealed,
        streams: streams
            .into_iter()
            .map(|(name, behind, failures)| (name, (behind, failures)))
            .collect(),
    })
}

fn register(snapshot: &Arc<Mutex<Snapshot>>) {
    let meter = global::meter("huxley");
    let s = snapshot.clone();
    let _ = meter
        .i64_observable_gauge("huxley.jobs")
        .with_description("Workflow jobs by kind and state (due, running, scheduled, dead)")
        .with_callback(move |o| {
            let s = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for ((kind, state), n) in &s.jobs {
                o.observe(
                    *n,
                    &[
                        KeyValue::new("kind", kind.clone()),
                        KeyValue::new("state", state.clone()),
                    ],
                );
            }
        })
        .build();
    let s = snapshot.clone();
    let _ = meter
        .f64_observable_gauge("huxley.jobs.oldest_due")
        .with_unit("s")
        .with_description("How long the oldest job waiting for a worker has waited")
        .with_callback(move |o| {
            let s = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            o.observe(s.oldest_due, &[]);
        })
        .build();
    let s = snapshot.clone();
    let _ = meter
        .i64_observable_gauge("huxley.audit.unsealed")
        .with_description("Audit records committed but not yet sealed into the hash chain")
        .with_callback(move |o| {
            let s = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            o.observe(s.unsealed, &[]);
        })
        .build();
    let s = snapshot.clone();
    let _ = meter
        .i64_observable_gauge("huxley.audit_stream.behind")
        .with_description("Sealed audit records a SIEM stream hasn't shipped yet")
        .with_callback(move |o| {
            let s = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (name, (behind, _)) in &s.streams {
                o.observe(*behind, &[KeyValue::new("stream", name.clone())]);
            }
        })
        .build();
    let s = snapshot.clone();
    let _ = meter
        .i64_observable_gauge("huxley.audit_stream.failures")
        .with_description("Failed attempts in a row to ship to a SIEM stream")
        .with_callback(move |o| {
            let s = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (name, (_, failures)) in &s.streams {
                o.observe(*failures, &[KeyValue::new("stream", name.clone())]);
            }
        })
        .build();
}

/// Reads the queue and audit gauges every 15 seconds until `shutdown`. Run
/// by the scheduler role (one reader is enough; with two, dashboards take
/// the max).
pub async fn run_gauges(engine: Engine, shutdown: CancellationToken) {
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    register(&snapshot);
    loop {
        match read(&engine).await {
            Ok(fresh) => {
                *snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
            }
            Err(e) => tracing::warn!(error = %e, "reading queue metrics failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(15)) => {}
        }
    }
}
