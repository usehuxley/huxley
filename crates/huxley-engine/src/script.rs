//! The `script` step (design doc §6.9): queues a library script for a
//! runner and parks. Runners claim tasks through the gateway and report
//! back; a report wakes the step job, which settles the step from the
//! task, so retries, output storage and secret masking work as for any
//! other step.

use huxley_crypto::Binding;
use huxley_db::{OrgScope, begin_scoped};
use std::collections::BTreeMap;

use huxley_scripts::Language;
use serde::{Deserialize, Serialize};
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
const REDACT_PURPOSE: &str = "script_task_redact";
/// Shorter values aren't scrubbed (they'd mangle ordinary text).
const MIN_REDACTED: usize = 4;
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

/// What's sealed with a task: everything the runner gets besides the
/// script itself.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Payload {
    inputs: Json,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Environment variable prefix → connection.
    #[serde(default)]
    connections: BTreeMap<String, Uuid>,
    /// Secret values in `inputs` and `env`, to scrub.
    #[serde(default)]
    redact: Vec<String>,
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
        let payload = match payload(&mut tx, org_id, input).await? {
            Ok(payload) => payload,
            Err(failure) => return Ok(Executed::Failed(failure)),
        };
        let payload = if record.tainted {
            let secrets = huxley_variables::for_run(&mut tx, &self.crypto, org_id)
                .await?
                .secrets;
            let given = serde_json::to_string(&(&payload.inputs, &payload.env)).unwrap_or_default();
            Payload {
                redact: secrets
                    .into_values()
                    .filter(|v| !v.is_empty() && given.contains(v.as_str()))
                    .collect(),
                ..payload
            }
        } else {
            payload
        };
        tx.commit().await?;
        if let Err(message) = huxley_scripts::check_inputs(&parameters, &payload.inputs) {
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
                &serde_json::to_vec(&payload).unwrap_or_default(),
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

/// The step's inputs, environment and connections, checked.
async fn payload(
    conn: &mut PgConnection,
    org_id: Uuid,
    input: &Json,
) -> EngineResult<Result<Payload, Json>> {
    let inputs = match &input["with"] {
        Json::Null => json!({}),
        Json::Object(_) => input["with"].clone(),
        _ => return Ok(Err(error("input", "`with` must be an object"))),
    };
    let env = input["env"]
        .as_object()
        .map(|env| {
            env.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| {
                    let text = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                    (k.clone(), text)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut connections = BTreeMap::new();
    if let Some(wanted) = input["connections"].as_object() {
        for (name, target) in wanted {
            let target = target.as_str().unwrap_or_default();
            match crate::action::connection_for(&mut *conn, org_id, target).await? {
                Ok(id) => connections.insert(name.clone(), id),
                Err(failure) => return Ok(Err(failure)),
            };
        }
    }
    Ok(Ok(Payload {
        inputs,
        env,
        connections,
        redact: Vec::new(),
    }))
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
    /// Its environment, credentials included.
    pub env: BTreeMap<String, String>,
    /// Values to scrub from its output and logs.
    pub redact: Vec<String>,
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

impl Engine {
    /// Hands the oldest queued task in one of `languages` to `runner_id`
    /// (never one whose run is over or being cancelled), with fresh
    /// connection credentials. A task whose credentials can't be had
    /// fails instead.
    pub async fn claim_script(
        &self,
        runner_id: Uuid,
        languages: &[Language],
    ) -> EngineResult<Option<ClaimedTask>> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let row: Option<(Uuid, Uuid, Language, Vec<u8>, i32, String)> = sqlx::query_as(
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
              RETURNING t.id, t.org_id, t.language, t.inputs_sealed, t.timeout_secs, v.source",
        )
        .bind(runner_id)
        .bind(languages)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some((id, org_id, language, sealed, timeout_secs, source)) = row else {
            return Ok(None);
        };
        let plain = self.crypto.decrypt(binding(PURPOSE, id), &sealed).await?;
        let payload: Payload = serde_json::from_slice(&plain).unwrap_or_default();
        let mut env = payload.env;
        let mut redact = payload.redact;
        for (name, connection) in &payload.connections {
            match self.script_credentials(name, *connection).await? {
                Ok(credentials) => {
                    env.extend(credentials.env);
                    redact.extend(credentials.secrets);
                }
                Err(failure) => {
                    let message = failure["message"].as_str().unwrap_or("no credentials");
                    self.fail_claimed(id, &format!("connection {name}: {message}"))
                        .await?;
                    return Ok(None);
                }
            }
        }
        redact.retain(|v| v.len() >= MIN_REDACTED);
        let sealed = self
            .crypto
            .encrypt(
                org_id,
                binding(REDACT_PURPOSE, id),
                &serde_json::to_vec(&redact).unwrap_or_default(),
            )
            .await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        sqlx::query("UPDATE script_tasks SET redact_sealed = $2 WHERE id = $1")
            .bind(id)
            .bind(&sealed)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(ClaimedTask {
            id,
            language,
            source,
            inputs: payload.inputs,
            env,
            redact,
            timeout_secs,
        }))
    }

    /// Fails a task that was just claimed, and wakes its step.
    async fn fail_claimed(&self, id: Uuid, message: &str) -> EngineResult<()> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "UPDATE script_tasks SET status = 'failed', error = $2, finished_at = now() \
              WHERE id = $1 AND status = 'running' RETURNING run_id, step_key",
        )
        .bind(id)
        .bind(message)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((run_id, step_key)) = row {
            wake(&mut tx, run_id, &step_key).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// What task `id`'s output and logs must not show.
    async fn redactions(&self, id: Uuid) -> EngineResult<Vec<String>> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let sealed: Option<Option<Vec<u8>>> =
            sqlx::query_scalar("SELECT redact_sealed FROM script_tasks WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        let Some(Some(sealed)) = sealed else {
            return Ok(Vec::new());
        };
        let plain = self
            .crypto
            .decrypt(binding(REDACT_PURPOSE, id), &sealed)
            .await?;
        Ok(serde_json::from_slice(&plain).unwrap_or_default())
    }
}

const fn binding(purpose: &str, record: Uuid) -> Binding<'_> {
    Binding { purpose, record }
}

/// Wakes a parked step job.
async fn wake(conn: &mut PgConnection, run_id: Uuid, step_key: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE jobs SET run_at = now() \
          WHERE run_id = $1 AND step_key = $2 AND kind = 'step' AND locked_by IS NULL",
    )
    .bind(run_id)
    .bind(step_key)
    .execute(&mut *conn)
    .await?;
    queue::notify(conn).await
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

impl Engine {
    /// Records `runner_id`'s result for task `id`, scrubbing the task's
    /// secrets from it (whatever the runner did), and wakes its step.
    pub async fn report_script(
        &self,
        runner_id: Uuid,
        id: Uuid,
        report: &Report,
    ) -> Result<(), ReportError> {
        let redact = self.redactions(id).await?;
        let scrub = |value: &Json| huxley_expr::redact(value, redact.iter().map(String::as_str));
        let scrub_text = |text: &str| match scrub(&Json::String(text.to_owned())) {
            Json::String(s) => s,
            _ => String::new(),
        };
        let logs = scrub_text(truncate(&report.logs, MAX_LOG_BYTES));
        let error = report.error.as_deref().map(scrub_text);
        let output = report
            .output
            .as_ref()
            .filter(|_| report.succeeded)
            .map(scrub);
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(crate::EngineError::from)?;
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
        .bind(output)
        .bind(error)
        .bind(report.exit_code)
        .bind(logs)
        .fetch_optional(&mut *tx)
        .await?;
        let (run_id, step_key) = row.ok_or(ReportError::NotYours)?;
        wake(&mut tx, run_id, &step_key).await?;
        tx.commit().await?;
        Ok(())
    }
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
