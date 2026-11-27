//! Starting, advancing, cancelling and finishing runs.

use std::collections::BTreeSet;

use huxley_crypto::Binding;
use huxley_db::{OrgScope, begin_scoped};
use huxley_hwf::Workflow;
use serde::Serialize;
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::journal::Journal;
use crate::payload::Payloads;
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

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
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

/// The purpose secret run inputs are encrypted under.
const SECRET_INPUTS: &str = "run_secret_inputs";

/// The inputs a workflow marks secret (`writeOnly: true`).
pub fn secret_input_names(document: &Json) -> BTreeSet<String> {
    document["spec"]["inputs"]["properties"]
        .as_object()
        .map(|p| {
            p.iter()
                .filter(|(_, schema)| schema["writeOnly"] == json!(true))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
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

/// Walks per advance that may stop to answer `lookup()`s.
const MAX_LOOKUP_ROUNDS: usize = 8;

impl Engine {
    /// Creates a run and queues its first advance, in the caller's
    /// transaction (which must be able to see `org_id`).
    pub async fn start_run(&self, conn: &mut PgConnection, start: StartRun) -> EngineResult<Uuid> {
        self.start_run_with_secrets(conn, start, &BTreeSet::new())
            .await
    }

    /// [`Self::start_run`], also keeping `secret` inputs encrypted (as if
    /// the workflow marked them `writeOnly`), e.g. a form's secret fields.
    pub async fn start_run_with_secrets(
        &self,
        conn: &mut PgConnection,
        start: StartRun,
        secret: &BTreeSet<String>,
    ) -> EngineResult<Uuid> {
        if start.version.is_some() == start.document.is_some() {
            return Err(EngineError::InvalidStart);
        }
        let id = huxley_core::new_id();
        let document = match (&start.document, start.workflow_id, start.version) {
            (Some(document), _, _) => Some(document.clone()),
            (None, Some(workflow), Some(version)) => sqlx::query_scalar(
                "SELECT document FROM workflow_versions WHERE workflow_id = $1 AND version = $2",
            )
            .bind(workflow)
            .bind(version)
            .fetch_optional(&mut *conn)
            .await?,
            _ => None,
        };
        // Secret (writeOnly) inputs are kept encrypted, not in `inputs`.
        let mut secret_names = document
            .as_ref()
            .map(secret_input_names)
            .unwrap_or_default();
        secret_names.extend(secret.iter().cloned());
        let (inputs, sealed) = self
            .seal_secret_inputs(start.org_id, id, &start.inputs, &secret_names)
            .await?;
        sqlx::query(
            "INSERT INTO runs (id, org_id, workflow_id, workflow_version, document, trigger_type, \
                    trigger, inputs, actor_id, inputs_sealed) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(id)
        .bind(start.org_id)
        .bind(start.workflow_id)
        .bind(start.version)
        .bind(&start.document)
        .bind(&start.trigger_type)
        .bind(&start.trigger)
        .bind(&inputs)
        .bind(start.actor_id)
        .bind(sealed)
        .execute(&mut *conn)
        .await?;
        // Integrations stay at the version current now for the whole run.
        if let Some(document) = document {
            crate::action::pin(&mut *conn, id, start.org_id, &document).await?;
        }
        queue::enqueue_advance(conn, start.org_id, id).await?;
        Ok(id)
    }

    /// Splits secret inputs off: the inputs to store (secrets replaced by a
    /// marker) and the secrets, encrypted.
    async fn seal_secret_inputs(
        &self,
        org_id: Uuid,
        run_id: Uuid,
        inputs: &Json,
        secret_names: &BTreeSet<String>,
    ) -> EngineResult<(Json, Option<Vec<u8>>)> {
        let Some(object) = inputs.as_object() else {
            return Ok((inputs.clone(), None));
        };
        let secrets: Map<String, Json> = object
            .iter()
            .filter(|(k, v)| secret_names.contains(*k) && !v.is_null())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if secrets.is_empty() {
            return Ok((inputs.clone(), None));
        }
        let mut shown = object.clone();
        for name in secrets.keys() {
            shown.insert(name.clone(), json!(huxley_expr::REDACTED));
        }
        let sealed = self
            .crypto
            .encrypt(
                org_id,
                Binding {
                    purpose: SECRET_INPUTS,
                    record: run_id,
                },
                &serde_json::to_vec(&Json::Object(secrets)).unwrap_or_default(),
            )
            .await?;
        Ok((Json::Object(shown), Some(sealed)))
    }

    /// A run's secret inputs, decrypted (empty if it has none).
    pub(crate) async fn secret_inputs(
        &self,
        conn: &mut PgConnection,
        run_id: Uuid,
    ) -> EngineResult<Map<String, Json>> {
        let sealed: Option<Option<Vec<u8>>> =
            sqlx::query_scalar("SELECT inputs_sealed FROM runs WHERE id = $1")
                .bind(run_id)
                .fetch_optional(&mut *conn)
                .await?;
        let Some(Some(sealed)) = sealed else {
            return Ok(Map::new());
        };
        let plain = self
            .crypto
            .decrypt(
                Binding {
                    purpose: SECRET_INPUTS,
                    record: run_id,
                },
                &sealed,
            )
            .await?;
        Ok(serde_json::from_slice(&plain).unwrap_or_default())
    }

    /// Every secret value a run can touch, for redaction: the
    /// organization's secrets and the run's secret inputs.
    pub(crate) async fn secret_values(
        &self,
        conn: &mut PgConnection,
        org_id: Uuid,
        run_id: Uuid,
    ) -> EngineResult<Vec<String>> {
        let mut values: Vec<String> = huxley_variables::for_run(&mut *conn, &self.crypto, org_id)
            .await?
            .secrets
            .into_values()
            .collect();
        values.extend(
            self.secret_inputs(conn, run_id)
                .await?
                .into_values()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)),
        );
        Ok(values)
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
            finish(
                &mut tx,
                &self.payloads,
                run_id,
                job.id,
                RunStatus::Cancelled,
                None,
                None,
            )
            .await?;
            queue::complete(&mut tx, job.id, worker).await?;
            tx.commit().await?;
            return Ok(());
        }

        let document = load_document(&mut tx, &run).await?;
        let workflow: Workflow =
            serde_json::from_value(document).map_err(|e| EngineError::Document(e.to_string()))?;
        let now = OffsetDateTime::now_utc();
        // A queued run waits while its workflow is at its concurrency limit.
        if run.status == RunStatus::Queued
            && let Some(limit) = workflow.spec.settings.as_ref().and_then(|s| s.concurrency)
            && at_limit(&mut tx, &run, run_id, limit).await?
        {
            queue::enqueue_advance_at(&mut tx, run.org_id, run_id, now + CONCURRENCY_RETRY).await?;
            queue::complete(&mut tx, job.id, worker).await?;
            tx.commit().await?;
            return Ok(());
        }
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
                    &self.payloads,
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
                    &self.payloads,
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
    ///
    /// Each round answers the lookups the previous walk asked for (at most
    /// [`huxley_expr::MAX_MISSING`] each).
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
        journal
            .hydrate(&self.crypto, &self.payloads, run_id)
            .await?;
        let org_id = run.org_id;
        let variables = huxley_variables::for_run(tx, &self.crypto, org_id).await?;
        let secret_inputs = self.secret_inputs(tx, run_id).await?;
        let mut inputs = run.inputs;
        let mut secrets = variables.secrets;
        if let Some(object) = inputs.as_object_mut() {
            for (name, value) in &secret_inputs {
                object.insert(name.clone(), value.clone());
                // So whatever a tainted step echoes gets masked too.
                let text = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                secrets.insert(format!("\u{0}input:{name}"), text);
            }
        }
        let mut ctx = RunContext {
            run_id,
            org_id,
            inputs,
            secret_inputs: secret_inputs.keys().cloned().collect(),
            trigger: run.trigger,
            vars: match run.vars {
                Json::Object(vars) => vars,
                _ => Map::new(),
            },
            meta: meta(workflow, run.workflow_version, started_at),
            actor: json!({ "id": run.actor_id }),
            org_vars: Json::Object(variables.vars),
            secrets,
            public_url: self.public_url.clone(),
            lookups: std::sync::Arc::default(),
        };
        // Walk; if expressions asked for lookups the run hasn't recorded,
        // answer them and walk again (the journal so far stays in memory).
        let mut known = crate::lookup::recorded(tx, run_id).await?;
        let mut schedules = Vec::new();
        let mut rounds = 0;
        let outcome = loop {
            ctx.lookups = std::sync::Arc::new(huxley_expr::Lookups::new(known.clone()));
            let mut walker = Walker::new(&self.expr, &mut ctx, &mut journal, now);
            let outcome = walker.run(workflow);
            schedules.append(&mut walker.schedules);
            let missing = ctx.lookups.take_missing();
            if missing.is_empty() {
                break outcome;
            }
            rounds += 1;
            if rounds > MAX_LOOKUP_ROUNDS {
                // Plenty for now: carry on in the next advance.
                queue::enqueue_advance(tx, org_id, run_id).await?;
                break outcome;
            }
            known.extend(self.answer_lookups(tx, run_id, org_id, missing).await?);
        };
        journal
            .persist(tx, run_id, org_id, &self.crypto, &self.payloads)
            .await?;
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

    /// Resumes a failed run from where it failed. Failed steps (and those the
    /// failure cancelled) run again as new attempts; failed control-flow
    /// steps reopen with their original decision; `on_error` steps are set
    /// aside so they can run again if needed. Returns false unless the run
    /// had failed.
    pub async fn resume_run(&self, conn: &mut PgConnection, run_id: Uuid) -> EngineResult<bool> {
        let run: Option<RunRow> = sqlx::query_as(
            "SELECT org_id, workflow_id, workflow_version, document, status, trigger, inputs, \
                    vars, actor_id, cancel_requested, started_at \
               FROM runs WHERE id = $1 FOR UPDATE",
        )
        .bind(run_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(run) = run.filter(|r| r.status == RunStatus::Failed) else {
            return Ok(false);
        };
        let workflow: Workflow = serde_json::from_value(load_document(conn, &run).await?)
            .map_err(|e| EngineError::Document(e.to_string()))?;
        let mut on_error = Vec::new();
        collect_ids(&workflow.spec.on_error, &mut on_error);

        // The latest attempt of each step key, for the updates below.
        let latest = "s.attempt = (SELECT max(attempt) FROM step_runs l \
                                    WHERE l.run_id = s.run_id AND l.step_key = s.step_key)";
        // Reopen failed (or, when the run failed, cancelled) containers,
        // keeping their decisions.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE step_runs s SET status = 'running', error = NULL, finished_at = NULL \
              WHERE s.run_id = $1 AND NOT s.superseded AND s.status IN ('failed', 'cancelled') \
                AND s.step_type IN ('if', 'switch', 'foreach', 'parallel', 'try') \
                AND NOT (s.step_id = ANY($2)) AND {latest}"
        )))
        .bind(run_id)
        .bind(&on_error)
        .execute(&mut *conn)
        .await?;
        // Set aside failed and cancelled steps, and everything on_error did.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE step_runs SET superseded = true \
              WHERE run_id = $1 AND NOT superseded \
                AND (step_id = ANY($2) OR step_key IN ( \
                     SELECT s.step_key FROM step_runs s \
                      WHERE s.run_id = $1 AND NOT s.superseded \
                        AND s.status IN ('failed', 'cancelled') AND {latest}))"
        )))
        .bind(run_id)
        .bind(&on_error)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE runs SET status = 'running', error = NULL, outputs = NULL, finished_at = NULL, \
                    started_at = NULL WHERE id = $1",
        )
        .bind(run_id)
        .execute(&mut *conn)
        .await?;
        // A lookup that failed (an unknown dataset, say) is asked again.
        crate::lookup::forget_failures(conn, run_id).await?;
        queue::enqueue_advance(conn, run.org_id, run_id).await?;
        Ok(true)
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

fn collect_ids(steps: &[huxley_hwf::Step], out: &mut Vec<String>) {
    for step in steps {
        out.push(step.id().to_owned());
        for (_, block) in step.blocks() {
            collect_ids(block, out);
        }
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
#[allow(clippy::too_many_arguments)]
async fn finish(
    conn: &mut PgConnection,
    payloads: &Payloads,
    run_id: Uuid,
    keep_job: Uuid,
    status: RunStatus,
    outputs: Option<Json>,
    error: Option<Json>,
) -> EngineResult<()> {
    crate::metrics::run_finished(status.as_str());
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
    .bind(&outputs)
    .bind(&error)
    .execute(&mut *conn)
    .await?;

    // Children still running stop with their parent.
    if status != RunStatus::Succeeded {
        let children: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "UPDATE runs SET cancel_requested = true \
              WHERE parent_run_id = $1 AND status IN ('queued', 'running') \
              RETURNING id, org_id",
        )
        .bind(run_id)
        .fetch_all(&mut *conn)
        .await?;
        for (child, org) in children {
            queue::enqueue_advance(conn, org, child).await?;
        }
    }

    // A subworkflow that was waited for completes its parent's step.
    let parent: Option<(Option<Uuid>, Option<String>, Option<Uuid>)> = sqlx::query_as(
        "SELECT r.parent_run_id, r.parent_step_key, p.org_id \
           FROM runs r LEFT JOIN runs p ON p.id = r.parent_run_id WHERE r.id = $1",
    )
    .bind(run_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((Some(parent_run), Some(parent_key), Some(parent_org))) = parent {
        let (step_status, step_output, step_error) = match status {
            RunStatus::Succeeded => ("succeeded", outputs.or(Some(json!({}))), None),
            other => (
                "failed",
                None,
                Some(json!({
                    "kind": "subworkflow",
                    "message": error
                        .as_ref()
                        .and_then(|e| e["message"].as_str())
                        .map_or_else(|| format!("the subworkflow run was {other:?}").to_lowercase(), str::to_owned),
                    "run_id": run_id,
                    "error": error,
                })),
            ),
        };
        let (step_output, output_ref) = payloads
            .store(parent_org, parent_run, step_output.as_ref())
            .await?;
        let updated: Option<(Uuid,)> = sqlx::query_as(
            "UPDATE step_runs s SET status = $3::step_status, output = $4, error = $5, \
                    output_ref = $6, finished_at = now() \
               FROM runs p \
              WHERE s.run_id = $1 AND s.step_key = $2 AND p.id = s.run_id \
                AND s.status = 'running' \
                AND s.attempt = (SELECT max(attempt) FROM step_runs \
                                  WHERE run_id = $1 AND step_key = $2) \
             RETURNING p.org_id",
        )
        .bind(parent_run)
        .bind(&parent_key)
        .bind(step_status)
        .bind(step_output)
        .bind(step_error)
        .bind(output_ref)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((parent_org,)) = updated {
            queue::enqueue_advance(conn, parent_org, parent_run).await?;
        }
    }
    Ok(())
}

/// How long a run waiting for a concurrency slot waits before checking again.
const CONCURRENCY_RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether `limit` runs of this run's workflow are already running in its org.
/// Serialized per workflow and org, so two advances can't both slip in.
async fn at_limit(
    conn: &mut PgConnection,
    run: &RunRow,
    run_id: Uuid,
    limit: u32,
) -> EngineResult<bool> {
    let Some(workflow_id) = run.workflow_id else {
        return Ok(false);
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text || $2::text))")
        .bind(workflow_id)
        .bind(run.org_id)
        .execute(&mut *conn)
        .await?;
    let running: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM runs \
          WHERE workflow_id = $1 AND org_id = $2 AND status = 'running' AND id <> $3",
    )
    .bind(workflow_id)
    .bind(run.org_id)
    .bind(run_id)
    .fetch_one(conn)
    .await?;
    Ok(running >= i64::from(limit))
}

/// How deep subworkflows may nest.
pub(crate) const MAX_SUBWORKFLOW_DEPTH: usize = 8;

/// Starts a subworkflow run for a parent's step, in the caller's
/// transaction. The child runs in the parent's org; the workflow is found
/// by key there or on the nearest ancestor that has it published. Returns
/// the child's run id, or a step error.
pub(crate) async fn start_child(
    engine: &Engine,
    conn: &mut PgConnection,
    org_id: Uuid,
    parent_run: Uuid,
    parent_key: &str,
    input: &Json,
) -> EngineResult<Result<Uuid, Json>> {
    let key = input["workflow"].as_str().unwrap_or_default();
    let inputs = match &input["inputs"] {
        Json::Null => json!({}),
        other => other.clone(),
    };
    // Depth: walk up the parent chain.
    let mut depth = 1;
    let mut cursor = Some(parent_run);
    while let Some(run) = cursor {
        let (parent,): (Option<Uuid>,) =
            sqlx::query_as("SELECT parent_run_id FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&mut *conn)
                .await?;
        cursor = parent;
        if cursor.is_some() {
            depth += 1;
        }
        if depth >= MAX_SUBWORKFLOW_DEPTH {
            return Ok(Err(error(
                "limit",
                format!("subworkflows nest at most {MAX_SUBWORKFLOW_DEPTH} deep"),
            )));
        }
    }
    // The child acts as whoever the parent acts as.
    let (parent_actor,): (Option<Uuid>,) =
        sqlx::query_as("SELECT actor_id FROM runs WHERE id = $1")
            .bind(parent_run)
            .fetch_one(&mut *conn)
            .await?;
    let found: Option<(Uuid, i32, Json)> = sqlx::query_as(
        "SELECT w.id, w.current_version, v.document \
           FROM workflows w \
           JOIN organizations o ON o.id = w.org_id \
           JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.current_version \
          WHERE w.key = $1 AND w.status = 'active' \
            AND o.path @> (SELECT path FROM organizations WHERE id = $2) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(key)
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((workflow_id, version, document)) = found else {
        return Ok(Err(error(
            "definition",
            format!("no published workflow {key:?} here or above"),
        )));
    };
    if let Err(e) = huxley_workflows::check_inputs(&document, &inputs) {
        let mut failure = error("inputs", e.to_string());
        if let huxley_workflows::WorkflowError::InvalidInputs(problems) = e {
            failure["problems"] = json!(problems);
        }
        return Ok(Err(failure));
    }
    let child = engine
        .start_run(
            conn,
            StartRun {
                org_id,
                workflow_id: Some(workflow_id),
                version: Some(version),
                document: None,
                trigger_type: "subworkflow".into(),
                trigger: json!({ "parent_run": parent_run, "parent_step": parent_key }),
                inputs,
                actor_id: parent_actor,
            },
        )
        .await?;
    sqlx::query("UPDATE runs SET parent_run_id = $2, parent_step_key = $3 WHERE id = $1")
        .bind(child)
        .bind(parent_run)
        .bind(parent_key)
        .execute(&mut *conn)
        .await?;
    Ok(Ok(child))
}

/// What an expression gives against a run (the editor's preview).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// The run it was evaluated against.
    pub run_id: Uuid,
    #[schema(value_type = Option<Object>)]
    pub value: Option<Json>,
    /// Why it couldn't be evaluated, e.g. a name that isn't there.
    pub error: Option<String>,
}

impl Engine {
    /// Evaluates `expression` against run `run_id` as a top-level step of
    /// it would see it: its inputs, trigger, the steps' recorded outputs,
    /// variables and run details. Nothing is decrypted: secret values stay
    /// `«redacted»`, and `secrets.*` isn't available.
    pub async fn preview_expression(
        &self,
        conn: &mut PgConnection,
        run_id: Uuid,
        expression: &Json,
    ) -> EngineResult<Option<Preview>> {
        let run: Option<RunRow> = sqlx::query_as(
            "SELECT org_id, workflow_id, workflow_version, document, status, trigger, inputs, \
                    vars, actor_id, cancel_requested, started_at \
               FROM runs WHERE id = $1",
        )
        .bind(run_id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(run) = run else {
            return Ok(None);
        };
        let document = load_document(&mut *conn, &run).await?;
        let workflow: Workflow =
            serde_json::from_value(document).map_err(|e| EngineError::Document(e.to_string()))?;
        let journal = Journal::load(&mut *conn, run_id).await?;
        let variables = huxley_variables::for_run(&mut *conn, &self.crypto, run.org_id).await?;
        let mut meta = meta(
            &workflow,
            run.workflow_version,
            run.started_at.unwrap_or_else(OffsetDateTime::now_utc),
        );
        meta.insert("id".into(), json!(run_id));
        meta.insert("vars".into(), run.vars.clone());
        let scope = huxley_expr::Scope::new()
            .with("inputs", &run.inputs)
            .with("trigger", &run.trigger)
            .with("steps", &journal.visible(&[]))
            .with("vars", &Json::Object(variables.vars))
            .with("run", &Json::Object(meta))
            .with("org", &json!({ "id": run.org_id }))
            .with("actor", &json!({ "id": run.actor_id }))
            .with("env", &json!({}))
            .with("connections", &json!({}));
        // The answers the run recorded; a preview never looks anything up.
        let lookups = std::sync::Arc::new(huxley_expr::Lookups::new(
            crate::lookup::recorded(&mut *conn, run_id).await?,
        ));
        let scope = scope.with_lookups(lookups.clone());
        let (value, error) = match self.expr.eval(expression, &scope) {
            Ok(evaluated) => (Some(evaluated.value), None),
            Err(_) if lookups.missing_count() > 0 => (
                None,
                Some(
                    "this run didn't look that up: lookup() answers are recorded as a run runs"
                        .into(),
                ),
            ),
            Err(e) => (None, Some(e.to_string())),
        };
        Ok(Some(Preview {
            run_id,
            value,
            error,
        }))
    }
}
