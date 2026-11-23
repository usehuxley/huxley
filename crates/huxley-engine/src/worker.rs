//! The worker loop: claim a job, process it while heartbeating its lease,
//! repeat. Idle workers sleep until `NOTIFY huxley_jobs` or the poll interval.

use std::sync::Arc;
use std::time::Duration;

use huxley_db::{OrgScope, begin_scoped};
use serde_json::json;
use sqlx::postgres::PgListener;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::queue::{self, Job};
use crate::walk::error;
use crate::{Engine, EngineError, EngineResult};

/// Engine settings (`[engine]` in `huxley.toml`, or `HUXLEY_ENGINE__*`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Jobs each worker process runs at once.
    pub concurrency: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { concurrency: 8 }
    }
}

impl From<&EngineConfig> for WorkerConfig {
    fn from(config: &EngineConfig) -> Self {
        Self {
            concurrency: config.concurrency.clamp(1, 256),
            ..Self::default()
        }
    }
}

/// Worker settings.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Jobs processed at once by this process.
    pub concurrency: usize,
    /// How long a claimed job stays ours without a heartbeat.
    pub lease: Duration,
    /// How often idle workers look for due jobs (timers, missed notifies).
    pub poll: Duration,
    /// Identifies this process in `jobs.locked_by`.
    pub id: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let mut suffix = [0u8; 4];
        getrandom::fill(&mut suffix).expect("the operating system RNG is available");
        Self {
            concurrency: 8,
            lease: Duration::from_secs(60),
            poll: Duration::from_secs(2),
            id: format!(
                "{}-{}-{:08x}",
                std::env::var("HOSTNAME").unwrap_or_else(|_| "huxley".into()),
                std::process::id(),
                u32::from_le_bytes(suffix)
            ),
        }
    }
}

pub(crate) async fn claim_one(
    engine: &Engine,
    worker: &str,
    lease: Duration,
) -> EngineResult<Option<Job>> {
    let mut tx = begin_scoped(&engine.db, &OrgScope::System).await?;
    let job = queue::claim(&mut tx, worker, 1, lease).await?.pop();
    tx.commit().await?;
    Ok(job)
}

/// Puts a job back after an error, or dead-letters it; a dead step job fails
/// its step so the run doesn't wait forever.
pub(crate) async fn record_failure(
    engine: &Engine,
    job: &Job,
    failure: &EngineError,
) -> EngineResult<()> {
    tracing::warn!(job = %job.id, kind = %job.kind, attempts = job.attempts, error = %failure, "job failed");
    let mut tx = begin_scoped(&engine.db, &OrgScope::System).await?;
    let dead = queue::fail(&mut tx, job, &failure.to_string()).await?;
    if dead {
        tracing::error!(job = %job.id, kind = %job.kind, "job dead-lettered after repeated failures");
        if let (queue::kind::STEP, Some(org_id), Some(run_id), Some(key)) = (
            job.kind.as_str(),
            job.org_id,
            job.run_id,
            job.step_key.as_deref(),
        ) {
            sqlx::query(
                "UPDATE step_runs SET status = 'failed', error = $3, finished_at = now() \
                  WHERE run_id = $1 AND step_key = $2 AND status IN ('running', 'waiting')",
            )
            .bind(run_id)
            .bind(key)
            .bind(json!(error("infrastructure", failure.to_string())))
            .execute(&mut *tx)
            .await?;
            queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Runs workers until `shutdown`. Also keeps run partitions created ahead.
pub async fn run_workers(engine: Engine, config: WorkerConfig, shutdown: CancellationToken) {
    let wake = Arc::new(Notify::new());
    let mut tasks = JoinSet::new();
    tasks.spawn(listen(engine.clone(), wake.clone(), shutdown.clone()));
    tasks.spawn(maintain_partitions(engine.clone(), shutdown.clone()));
    for n in 0..config.concurrency.max(1) {
        let worker = format!("{}/{n}", config.id);
        tasks.spawn(work(
            engine.clone(),
            config.clone(),
            worker,
            wake.clone(),
            shutdown.clone(),
        ));
    }
    tracing::info!(concurrency = config.concurrency, id = %config.id, "workflow workers started");
    while tasks.join_next().await.is_some() {}
    tracing::info!("workflow workers stopped");
}

async fn work(
    engine: Engine,
    config: WorkerConfig,
    worker: String,
    wake: Arc<Notify>,
    shutdown: CancellationToken,
) {
    while !shutdown.is_cancelled() {
        let job = match claim_one(&engine, &worker, config.lease).await {
            Ok(job) => job,
            Err(e) => {
                tracing::error!(error = %e, "claiming jobs failed");
                None
            }
        };
        let Some(job) = job else {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = wake.notified() => {}
                () = tokio::time::sleep(config.poll) => {}
            }
            continue;
        };
        // Keep the lease while the job runs; a lost lease means another
        // worker may have it, which the handlers tolerate.
        let heartbeat = tokio::spawn(heartbeat(
            engine.clone(),
            job.id,
            worker.clone(),
            config.lease,
        ));
        let result = engine.process(&job, &worker).await;
        heartbeat.abort();
        if let Err(failure) = result
            && let Err(e) = record_failure(&engine, &job, &failure).await
        {
            tracing::error!(error = %e, job = %job.id, "recording a job failure failed");
        }
    }
}

async fn heartbeat(engine: Engine, job: uuid::Uuid, worker: String, lease: Duration) {
    let mut interval = tokio::time::interval(lease / 3);
    interval.tick().await;
    loop {
        interval.tick().await;
        let beat = async {
            let mut tx = begin_scoped(&engine.db, &OrgScope::System).await?;
            let held = queue::heartbeat(&mut tx, job, &worker, lease).await?;
            tx.commit().await?;
            Ok::<_, EngineError>(held)
        };
        match beat.await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(%job, "lost the lease on a running job");
                return;
            }
            Err(e) => tracing::warn!(error = %e, %job, "heartbeat failed"),
        }
    }
}

/// Wakes idle workers on `NOTIFY huxley_jobs`, reconnecting as needed.
async fn listen(engine: Engine, wake: Arc<Notify>, shutdown: CancellationToken) {
    while !shutdown.is_cancelled() {
        let mut listener = match PgListener::connect_with(&engine.db).await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::warn!(error = %e, "job listener can't connect; polling only");
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(5)) => continue,
                }
            }
        };
        if let Err(e) = listener.listen(queue::CHANNEL).await {
            tracing::warn!(error = %e, "LISTEN failed");
            continue;
        }
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                received = listener.recv() => match received {
                    Ok(_) => wake.notify_waiters(),
                    Err(e) => {
                        tracing::warn!(error = %e, "job listener disconnected");
                        break;
                    }
                },
            }
        }
    }
}

/// Creates next months' run partitions now and twice a day.
async fn maintain_partitions(engine: Engine, shutdown: CancellationToken) {
    loop {
        let created = async {
            let mut tx = begin_scoped(&engine.db, &OrgScope::System).await?;
            sqlx::query("SELECT huxley_ensure_run_partitions(2)")
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok::<_, EngineError>(())
        };
        if let Err(e) = created.await {
            tracing::error!(error = %e, "creating run partitions failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(12 * 3600)) => {}
        }
    }
}
