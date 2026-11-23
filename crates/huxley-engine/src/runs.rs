//! Reading runs and their steps, for the API and the run views.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{RunStatus, StepStatus};

/// `pg_notify` channel signalled with a run's id whenever the run or one of
/// its steps changes (a trigger on `runs` and `step_runs`).
pub const RUN_CHANGES: &str = "huxley_run_changes";

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workflow_id: Option<Uuid>,
    /// `null` for a test run of the draft.
    pub workflow_version: Option<i32>,
    pub status: RunStatus,
    pub trigger_type: String,
    pub actor_id: Option<Uuid>,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub error: Option<Json>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

/// The latest attempt of a step.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct StepRunView {
    /// Step id plus loop indices, e.g. `add_member[2]`.
    pub step_key: String,
    pub step_id: String,
    pub step_type: String,
    pub attempt: i32,
    pub status: StepStatus,
    /// The evaluated input (or a control-flow step's decision); `«redacted»`
    /// when it came from a secret.
    #[schema(value_type = Option<serde_json::Value>)]
    pub input: Option<Json>,
    #[schema(value_type = Option<serde_json::Value>)]
    pub output: Option<Json>,
    #[schema(value_type = Option<serde_json::Value>)]
    pub error: Option<Json>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    #[serde(flatten)]
    pub summary: RunSummary,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub inputs: Json,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub trigger: Json,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub outputs: Option<Json>,
    pub cancel_requested: bool,
    /// Every step that has started, in the order it started.
    pub steps: Vec<StepRunView>,
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub workflow_id: Option<Uuid>,
    pub status: Option<RunStatus>,
    /// Runs older than this one (keyset pagination; ids are time-ordered).
    pub before: Option<Uuid>,
    pub limit: i64,
}

const SUMMARY: &str = "id, org_id, workflow_id, workflow_version, status, trigger_type, actor_id, \
                       error, created_at, started_at, finished_at";

/// Runs in the caller's scope, newest first.
pub async fn list(conn: &mut PgConnection, filter: &RunFilter) -> sqlx::Result<Vec<RunSummary>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM runs \
          WHERE ($1::uuid IS NULL OR workflow_id = $1) \
            AND ($2::run_status IS NULL OR status = $2) \
            AND ($3::uuid IS NULL OR id < $3) \
          ORDER BY id DESC LIMIT $4"
    )))
    .bind(filter.workflow_id)
    .bind(filter.status)
    .bind(filter.before)
    .bind(filter.limit.clamp(1, 200))
    .fetch_all(conn)
    .await
}

/// A run with its steps, or `None` if it isn't in the caller's scope.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<RunDetail>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        summary: RunSummary,
        inputs: Json,
        trigger: Json,
        outputs: Option<Json>,
        cancel_requested: bool,
    }
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY}, inputs, trigger, outputs, cancel_requested FROM runs WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let steps: Vec<StepRunView> = sqlx::query_as(
        "SELECT * FROM ( \
            SELECT DISTINCT ON (step_key) step_key, step_id, step_type, attempt, status, input, \
                   output, error, started_at, finished_at, \
                   (SELECT min(started_at) FROM step_runs f \
                     WHERE f.run_id = s.run_id AND f.step_key = s.step_key) AS first_started \
              FROM step_runs s WHERE run_id = $1 AND NOT superseded \
             ORDER BY step_key, attempt DESC) latest \
          ORDER BY first_started, step_key",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(Some(RunDetail {
        summary: row.summary,
        inputs: row.inputs,
        trigger: row.trigger,
        outputs: row.outputs,
        cancel_requested: row.cancel_requested,
        steps,
    }))
}

/// Which half of a step attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PayloadField {
    Input,
    Output,
}

/// Where a step attempt's spilled input or output is: `Ok(None)` if the
/// attempt isn't in the caller's scope, `Ok(Some(None))` if that value
/// wasn't spilled.
pub async fn payload_ref(
    conn: &mut PgConnection,
    run_id: Uuid,
    step_key: &str,
    attempt: i32,
    field: PayloadField,
) -> sqlx::Result<Option<Option<String>>> {
    let column = match field {
        PayloadField::Input => "input_ref",
        PayloadField::Output => "output_ref",
    };
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT {column} FROM step_runs WHERE run_id = $1 AND step_key = $2 AND attempt = $3"
    )))
    .bind(run_id)
    .bind(step_key)
    .bind(attempt)
    .fetch_optional(conn)
    .await
}
