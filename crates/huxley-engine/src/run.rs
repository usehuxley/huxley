//! Starting, advancing, cancelling and finishing runs.

use huxley_db::{OrgScope, begin_scoped};
use huxley_hwf::Workflow;
use serde::Serialize;
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::journal::Journal;
use crate::queue::{self, Job};
use crate::walk::{Outcome, RunContext, Walker, error};
use crate::{Engine, EngineError, EngineResult};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, utoipa::ToSchema, sqlx::Type,
)]
#[sqlx(type_name = "run_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// What to run.
#[derive(Debug, Clone)]
pub struct StartRun {
    pub org_id: Uuid,
    pub workflow_id: Option<Uuid>,
    /// A published version; or `None` with `document` for a test run.
    pub version: Option<i32>,
    pub document: Option<Json>,
    pub trigger_type: String,
    pub trigger: Json,
    pub inputs: Json,
    pub actor_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
struct RunRow {
    org_id: Uuid,
    workflow_id: Option<Uuid>,
    workflow_version: Option<i32>,
    document: Option<Json>,
    status: RunStatus,
    trigger: Json,
    inputs: Json,
    vars: Json,
    actor_id: Option<Uuid>,
    cancel_requested: bool,
    started_at: Option<OffsetDateTime>,
}

impl Engine {
    /// Creates a run and queues its first advance, in the caller's
    /// transaction (which must be able to see `org_id`).
    pub async fn start_run(&self, conn: &mut PgConnection, start: StartRun) -> EngineResult<Uuid> {
        if start.version.is_some() == start.document.is_some() {
            return Err(EngineError::InvalidStart);
        }
        let id = huxley_core::new_id();
        sqlx::query(
            "INSERT INTO runs (id, org_id, workflow_id, workflow_version, document, trigger_type, \
                    trigger, inputs, actor_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(start.org_id)
        .bind(start.workflow_id)
        .bind(start.version)
        .bind(&start.document)
        .bind(&start.trigger_type)
        .bind(&start.trigger)
        .bind(&start.inputs)
        .bind(start.actor_id)
        .execute(&mut *conn)
        .await?;
        queue::enqueue_advance(conn, start.org_id, id).await?;
        Ok(id)
    }

    /// Asks a queued or running run to stop. Returns false if it had
    /// already finished.
    pub async fn cancel_run(&self, conn: &mut PgConnection, run_id: Uuid) -> EngineResult<bool> {
        let org: Option<(Uuid,)> = sqlx::query_as(
            "UPDATE runs SET cancel_requested = true \
              WHERE id = $1 AND status IN ('queued', 'running') RETURNING org_id",
        )
        .bind(run_id)
        .fetch_optional(&mut *conn)
        .await?;
        match org {
            Some((org_id,)) => {
                queue::enqueue_advance(conn, org_id, run_id).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn advance(&self, job: &Job, worker: &str) -> EngineResult<()> {
        let run_id = job
            .run_id
            .ok_or_else(|| EngineError::Document("advance job without a run".into()))?;
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        let run: Option<RunRow> = sqlx::query_as(
            "SELECT org_id, workflow_id, workflow_version, document, status, trigger, inputs, \
                        vars, actor_id, cancel_requested, started_at \
                   FROM runs WHERE id = $1 FOR UPDATE",
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(run) = run.filter(|r| !r.status.is_finished()) else {
            queue::complete(&mut tx, job.id, worker).await?;
            tx.commit().await?;
            return Ok(());
        };
        if run.cancel_requested {
            finish(&mut tx, run_id, job.id, RunStatus::Cancelled, None, None).await?;
            queue::complete(&mut tx, job.id, worker).await?;
            tx.commit().await?;
            return Ok(());
        }

        let document = load_document(&mut tx, &run).await?;
        let workflow: Workflow =
            serde_json::from_value(document).map_err(|e| EngineError::Document(e.to_string()))?;
        let now = OffsetDateTime::now_utc();
        let started_at = run.started_at.unwrap_or(now);
        let timeout = workflow
            .spec
            .settings
            .as_ref()
            .and_then(|s| s.timeout.as_deref())
            .and_then(huxley_hwf::duration::parse);
        if run.started_at.is_none()
            && let Some(timeout) = timeout
        {
            queue::enqueue_deadline(&mut tx, run.org_id, run_id, started_at + timeout).await?;
        }

        let outcome = if timeout.is_some_and(|t| now >= started_at + t) {
            Outcome::Failed(error("timeout", "the run took longer than its timeout"))
        } else {
            self.walk(&mut tx, run_id, run, &workflow, started_at, now)
                .await?
        };

        match outcome {
            Outcome::Pending => {}
            Outcome::Succeeded(outputs) => {
                finish(
                    &mut tx,
                    run_id,
                    job.id,
                    RunStatus::Succeeded,
                    Some(outputs),
                    None,
                )
                .await?;
            }
            Outcome::Failed(error) => {
                finish(
                    &mut tx,
                    run_id,
                    job.id,
                    RunStatus::Failed,
                    None,
                    Some(error),
                )
                .await?;
            }
        }
        queue::complete(&mut tx, job.id, worker).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Walks the run against its journal and persists what changed.
    async fn walk(
        &self,
        tx: &mut PgConnection,
        run_id: Uuid,
        run: RunRow,
        workflow: &Workflow,
        started_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> EngineResult<Outcome> {
        let mut journal = Journal::load(tx, run_id).await?;
        journal.unseal(&self.crypto, run_id).await?;
        let org_id = run.org_id;
        let variables = huxley_variables::for_run(tx, &self.crypto, org_id).await?;
        let mut ctx = RunContext {
            run_id,
            org_id,
            inputs: run.inputs,
            trigger: run.trigger,
            vars: match run.vars {
                Json::Object(vars) => vars,
                _ => Map::new(),
            },
            meta: meta(workflow, run.workflow_version, started_at),
            actor: json!({ "id": run.actor_id }),
            org_vars: Json::Object(variables.vars),
            secrets: variables.secrets,
        };
        let mut walker = Walker::new(&self.expr, &mut ctx, &mut journal, now);
        let outcome = walker.run(workflow);
        let schedules = std::mem::take(&mut walker.schedules);
        journal.persist(tx, run_id, org_id, &self.crypto).await?;
        for schedule in schedules {
            queue::enqueue_step(tx, org_id, run_id, &schedule.step_key, schedule.run_at).await?;
        }
        sqlx::query(
            "UPDATE runs SET status = 'running', started_at = COALESCE(started_at, $2), \
                    vars = $3 WHERE id = $1",
        )
        .bind(run_id)
        .bind(started_at)
        .bind(Json::Object(ctx.vars))
        .execute(&mut *tx)
        .await?;
        Ok(outcome)
    }

    /// The run's deadline passed: advance it, which fails it.
    pub(crate) async fn deadline(&self, job: &Job, worker: &str) -> EngineResult<()> {
        let mut tx = begin_scoped(&self.db, &OrgScope::System).await?;
        if let (Some(org_id), Some(run_id)) = (job.org_id, job.run_id) {
            queue::enqueue_advance(&mut tx, org_id, run_id).await?;
        }
        queue::complete(&mut tx, job.id, worker).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn load_document(conn: &mut PgConnection, run: &RunRow) -> EngineResult<Json> {
    if let Some(document) = &run.document {
        return Ok(document.clone());
    }
    let (document,): (Json,) = sqlx::query_as(
        "SELECT document FROM workflow_versions WHERE workflow_id = $1 AND version = $2",
    )
    .bind(run.workflow_id)
    .bind(run.workflow_version)
    .fetch_one(conn)
    .await?;
    Ok(document)
}

fn meta(
    workflow: &Workflow,
    version: Option<i32>,
    started_at: OffsetDateTime,
) -> Map<String, Json> {
    let mut meta = Map::new();
    meta.insert(
        "workflow".into(),
        json!({
            "key": workflow.metadata.key,
            "name": workflow.metadata.name,
            "version": version,
        }),
    );
    meta.insert(
        "started_at".into(),
        json!(
            started_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        ),
    );
    meta.insert("test".into(), json!(version.is_none()));
    meta
}

/// Ends a run: any steps still in flight are cancelled and its other queued
/// jobs removed.
async fn finish(
    conn: &mut PgConnection,
    run_id: Uuid,
    keep_job: Uuid,
    status: RunStatus,
    outputs: Option<Json>,
    error: Option<Json>,
) -> EngineResult<()> {
    sqlx::query(
        "UPDATE step_runs SET status = 'cancelled', finished_at = now() \
          WHERE run_id = $1 AND status IN ('running', 'waiting')",
    )
    .bind(run_id)
    .execute(&mut *conn)
    .await?;
    queue::delete_for_run(conn, run_id, Some(keep_job)).await?;
    sqlx::query(
        "UPDATE runs SET status = $2, outputs = $3, error = $4, finished_at = now(), \
                started_at = COALESCE(started_at, now()) WHERE id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(outputs)
    .bind(error)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
