//! The `script` step (design doc §6.9): queues a library script for a
//! runner and parks. Runners claim tasks through the gateway and report
//! back; a report wakes the step job, which settles the step from the
//! task, so retries, output storage and secret masking work as for any
//! other step.

use huxley_crypto::{Binding, Crypto};
use huxley_db::{OrgScope, begin_scoped};
use huxley_scripts::Language;
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::journal::Record;
use crate::step::Executed;
use crate::walk::error;
use crate::{Engine, EngineResult, queue};

/// How long a script may take (queueing included) without a `timeout`.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Logs kept per task.
pub const MAX_LOG_BYTES: usize = 64 * 1024;
const PURPOSE: &str = "script_task_inputs";
/// The end of the logs a failed step's error carries.
const ERROR_LOG_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "script_task_status", rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    status: TaskStatus,
    created_at: OffsetDateTime,
    timeout_secs: i32,
    output: Option<Json>,
    error: Option<String>,
    exit_code: Option<i32>,
    logs: Option<String>,
}

impl Engine {
    /// The step job of a `script` step.
    pub(crate) async fn park_script(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        record: &Record,
        input: &Json,
        timeout: std::time::Duration,
    ) -> EngineResult<Executed> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let task = task_for(&mut tx, run_id, record).await?;
        tx.commit().await?;
        let Some(task) = task else {
            return self
                .queue_script(run_id, org_id, record, input, timeout)
                .await;
        };
        let deadline = task.created_at + Duration::seconds(i64::from(task.timeout_secs));
        match task.status {
            TaskStatus::Succeeded => Ok(Executed::Done(task.output.unwrap_or(Json::Null))),
            TaskStatus::Failed => Ok(Executed::Failed(json!({
                "kind": "script",
                "message": task.error.unwrap_or_else(|| "the script failed".into()),
                "exit_code": task.exit_code,
                "logs": tail(task.logs.as_deref().unwrap_or_default(), ERROR_LOG_BYTES),
            }))),
            TaskStatus::Queued | TaskStatus::Running if OffsetDateTime::now_utc() >= deadline => {
                let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
                let message = if task.status == TaskStatus::Queued {
                    "no runner picked the script up in time"
                } else {
                    "the script didn't finish in time"
                };
                sqlx::query(
                    "UPDATE script_tasks SET status = 'failed', error = $4, finished_at = now() \
                      WHERE run_id = $1 AND step_key = $2 AND attempt = $3 \
                        AND status IN ('queued', 'running')",
                )
                .bind(run_id)
                .bind(&record.step_key)
                .bind(record.attempt)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Executed::Failed(error("timeout", message)))
            }
            TaskStatus::Queued | TaskStatus::Running => Ok(Executed::NotYet(deadline)),
        }
    }

    async fn queue_script(
        &self,
        run_id: Uuid,
        org_id: Uuid,
        record: &Record,
        input: &Json,
        timeout: std::time::Duration,
    ) -> EngineResult<Executed> {
        let key = input["script"].as_str().unwrap_or_default();
        let inputs = match &input["with"] {
            Json::Null => json!({}),
            Json::Object(_) => input["with"].clone(),
            _ => return Ok(Executed::Failed(error("input", "`with` must be an object"))),
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let Some(script) = huxley_scripts::find(&mut tx, org_id, key)
            .await
            .map_err(|e| crate::EngineError::Document(e.to_string()))?
        else {
            return Ok(Executed::Failed(error(
                "definition",
                format!("there's no script `{key}` here"),
            )));
        };
        let (_, parameters) =
            huxley_scripts::version_source(&mut tx, script.id, script.current_version)
                .await
                .map_err(|e| crate::EngineError::Document(e.to_string()))?;
        tx.commit().await?;
        if let Err(message) = huxley_scripts::check_inputs(&parameters, &inputs) {
            return Ok(Executed::Failed(error("input", message)));
        }
        let id = huxley_core::new_id();
        let sealed = self
            .crypto
            .encrypt(
                org_id,
                Binding {
                    purpose: PURPOSE,
                    record: id,
                },
                inputs.to_string().as_bytes(),
            )
            .await?;
        let timeout_secs = i32::try_from(timeout.as_secs()).unwrap_or(i32::MAX).max(1);
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        sqlx::query(
            "INSERT INTO script_tasks (id, run_id, org_id, step_key, attempt, script_id, \
                    script_version, language, inputs_sealed, timeout_secs) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (run_id, step_key, attempt) DO NOTHING",
        )
        .bind(id)
        .bind(run_id)
        .bind(org_id)
        .bind(&record.step_key)
        .bind(record.attempt)
        .bind(script.id)
        .bind(script.current_version)
        .bind(script.language)
        .bind(&sealed)
        .bind(timeout_secs)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE step_runs SET status = 'waiting' \
              WHERE run_id = $1 AND step_key = $2 AND attempt = $3 AND status = 'running'",
        )
        .bind(run_id)
        .bind(&record.step_key)
        .bind(record.attempt)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Executed::NotYet(
            OffsetDateTime::now_utc() + Duration::seconds(i64::from(timeout_secs)),
        ))
    }
}

async fn task_for(
    conn: &mut PgConnection,
    run_id: Uuid,
    record: &Record,
) -> sqlx::Result<Option<TaskRow>> {
    sqlx::query_as(
        "SELECT status, created_at, timeout_secs, output, error, exit_code, logs FROM script_tasks \
          WHERE run_id = $1 AND step_key = $2 AND attempt = $3",
    )
    .bind(run_id)
    .bind(&record.step_key)
    .bind(record.attempt)
    .fetch_optional(conn)
    .await
}

/// A task handed to a runner.
pub struct ClaimedTask {
    pub id: Uuid,
    pub language: Language,
    pub source: String,
    /// The script's parameters, in the clear.
    pub inputs: Json,
    pub timeout_secs: i32,
}

impl std::fmt::Debug for ClaimedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedTask")
            .field("id", &self.id)
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

/// Hands the oldest queued task in one of `languages` to `runner_id`
/// (never one whose run is over or being cancelled).
pub async fn claim(
    db: &sqlx::PgPool,
    crypto: &Crypto,
    runner_id: Uuid,
    languages: &[Language],
) -> EngineResult<Option<ClaimedTask>> {
    let mut tx = begin_scoped(db, &OrgScope::System).await?;
    let row: Option<(Uuid, Language, Vec<u8>, i32, String)> = sqlx::query_as(
        "UPDATE script_tasks t SET status = 'running', runner_id = $1, claimed_at = now() \
           FROM script_versions v \
          WHERE t.id = (SELECT id FROM script_tasks \
                         WHERE status = 'queued' AND language = ANY($2) \
                           AND created_at + make_interval(secs => timeout_secs) > now() \
                           AND EXISTS (SELECT 1 FROM runs r WHERE r.id = script_tasks.run_id \
                                         AND r.status IN ('queued', 'running') \
                                         AND NOT r.cancel_requested) \
                         ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
            AND v.script_id = t.script_id AND v.version = t.script_version \
          RETURNING t.id, t.language, t.inputs_sealed, t.timeout_secs, v.source",
    )
    .bind(runner_id)
    .bind(languages)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((id, language, sealed, timeout_secs, source)) = row else {
        return Ok(None);
    };
    let plain = crypto
        .decrypt(
            Binding {
                purpose: PURPOSE,
                record: id,
            },
            &sealed,
        )
        .await?;
    let inputs = serde_json::from_slice(&plain).unwrap_or_else(|_| json!({}));
    Ok(Some(ClaimedTask {
        id,
        language,
        source,
        inputs,
        timeout_secs,
    }))
}

/// What a runner reports about a task.
#[derive(Debug, Clone)]
pub struct Report {
    pub succeeded: bool,
    pub output: Option<Json>,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub logs: String,
}

/// Why a report wasn't accepted.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("no such task is running on this runner")]
    NotYours,
    #[error(transparent)]
    Engine(#[from] crate::EngineError),
}

impl From<sqlx::Error> for ReportError {
    fn from(e: sqlx::Error) -> Self {
        Self::Engine(e.into())
    }
}

/// Records `runner_id`'s result for task `id`, and wakes its step.
pub async fn report(
    db: &sqlx::PgPool,
    runner_id: Uuid,
    id: Uuid,
    report: &Report,
) -> Result<(), ReportError> {
    let mut tx = begin_scoped(db, &OrgScope::System)
        .await
        .map_err(crate::EngineError::from)?;
    let logs = truncate(&report.logs, MAX_LOG_BYTES);
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE script_tasks SET status = $3::script_task_status, output = $4, error = $5, \
                exit_code = $6, logs = $7, finished_at = now() \
          WHERE id = $1 AND runner_id = $2 AND status = 'running' \
          RETURNING run_id, step_key",
    )
    .bind(id)
    .bind(runner_id)
    .bind(if report.succeeded {
        "succeeded"
    } else {
        "failed"
    })
    .bind(report.output.as_ref().filter(|_| report.succeeded))
    .bind(report.error.as_deref())
    .bind(report.exit_code)
    .bind(logs)
    .fetch_optional(&mut *tx)
    .await?;
    let (run_id, step_key) = row.ok_or(ReportError::NotYours)?;
    // Wake the parked step job.
    sqlx::query(
        "UPDATE jobs SET run_at = now() \
          WHERE run_id = $1 AND step_key = $2 AND kind = 'step' AND locked_by IS NULL",
    )
    .bind(run_id)
    .bind(&step_key)
    .execute(&mut *tx)
    .await?;
    queue::notify(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

/// At most `max` bytes of `s`, cut at a character boundary.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// At most the last `max` bytes of `s`, cut at a character boundary.
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

#[cfg(test)]
mod tests {
    use super::{tail, truncate};

    #[test]
    fn truncates_at_a_character_boundary() {
        assert_eq!(truncate("héllo", 2), "h");
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(tail("héllo", 4), "llo");
        assert_eq!(tail("hello", 10), "hello");
    }
}
