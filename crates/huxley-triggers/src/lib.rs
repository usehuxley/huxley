//! Workflow triggers at runtime (design doc §6.5).
//!
//! - [`run_scheduler`]: the scheduler role. One process at a time is leader
//!   (a PostgreSQL advisory lock held on a dedicated connection); it starts
//!   runs for schedules that are due.
//! - [`webhook`]: verifying webhook requests (HMAC, bearer, basic).
//! - [`find_webhook`] and [`start`]: what the webhook endpoint calls.

pub mod webhook;

use std::time::Duration;

use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, StartRun};
use huxley_hwf::{Trigger, WebhookTrigger};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Scheduler leadership: `pg_try_advisory_lock` key.
const LEADER_LOCK: i64 = 0x6875_786c_6579_0001; // "huxley" + 1
const TICK: Duration = Duration::from_secs(5);
const BATCH: i64 = 100;

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Engine(#[from] huxley_engine::EngineError),
    #[error(transparent)]
    Workflow(#[from] huxley_workflows::WorkflowError),
    #[error(transparent)]
    Scope(#[from] huxley_db::DbError),
    #[error(transparent)]
    Variables(#[from] huxley_variables::VariableError),
}

pub type TriggerResult<T> = Result<T, TriggerError>;

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    workflow_id: Uuid,
    org_id: Uuid,
    trigger_id: String,
    config: Json,
    next_run_at: OffsetDateTime,
    current_version: Option<i32>,
    active: bool,
}

/// Starts runs for every schedule that is due. Returns how many started.
/// Missed occurrences (the scheduler was down) run once, then the schedule
/// continues from now.
pub async fn tick(engine: &Engine) -> TriggerResult<usize> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let due: Vec<Due> = sqlx::query_as(
        "SELECT t.id, t.workflow_id, t.org_id, t.trigger_id, t.config, t.next_run_at, \
                w.current_version, w.status = 'active' AS active \
           FROM workflow_triggers t JOIN workflows w ON w.id = t.workflow_id \
          WHERE t.type = 'schedule' AND t.next_run_at <= now() \
          ORDER BY t.next_run_at LIMIT $1 FOR UPDATE OF t SKIP LOCKED",
    )
    .bind(BATCH)
    .fetch_all(&mut *tx)
    .await?;
    let now = OffsetDateTime::now_utc();
    let mut started = 0;
    for trigger in due {
        // Stored as the tagged `Trigger`.
        let schedule = match serde_json::from_value(trigger.config.clone()) {
            Ok(Trigger::Schedule(schedule)) => Some(schedule),
            _ => None,
        };
        let next = match &schedule {
            Some(schedule) => {
                let zone =
                    huxley_workflows::triggers::schedule_zone(&mut tx, trigger.org_id, schedule)
                        .await?;
                huxley_workflows::triggers::next_run(schedule, &zone, now)
            }
            None => None,
        };
        if let (Some(schedule), Some(version), true) =
            (&schedule, trigger.current_version, trigger.active)
        {
            let inputs = schedule.inputs.clone().unwrap_or_else(|| json!({}));
            if start_schedule(engine, &mut tx, &trigger, version, inputs).await? {
                started += 1;
            }
        }
        sqlx::query(
            "UPDATE workflow_triggers SET last_run_at = $2, next_run_at = $3, updated_at = now() \
              WHERE id = $1",
        )
        .bind(trigger.id)
        .bind(trigger.next_run_at)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(started)
}

async fn start_schedule(
    engine: &Engine,
    conn: &mut PgConnection,
    trigger: &Due,
    version: i32,
    inputs: Json,
) -> TriggerResult<bool> {
    let version_doc = huxley_workflows::version(conn, trigger.workflow_id, version).await?;
    if let Err(e) = huxley_workflows::check_inputs(&version_doc.document, &inputs) {
        tracing::warn!(
            workflow = %trigger.workflow_id, trigger = %trigger.trigger_id, error = %e,
            "scheduled run skipped: its inputs don't match the workflow's input schema"
        );
        return Ok(false);
    }
    let scheduled_for = trigger
        .next_run_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    engine
        .start_run(
            conn,
            StartRun {
                org_id: trigger.org_id,
                workflow_id: Some(trigger.workflow_id),
                version: Some(version),
                document: None,
                trigger_type: "schedule".into(),
                trigger: json!({ "id": trigger.trigger_id, "scheduled_for": scheduled_for }),
                inputs,
                actor_id: None,
            },
        )
        .await?;
    Ok(true)
}

/// Runs the scheduler until `shutdown`: waits to become leader, then ticks.
pub async fn run_scheduler(engine: Engine, shutdown: CancellationToken) {
    while !shutdown.is_cancelled() {
        match lead(&engine, &shutdown).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "scheduler lost its database connection"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
    }
}

/// Holds leadership (if it can get it) and ticks until shutdown or until the
/// connection holding the lock fails.
async fn lead(engine: &Engine, shutdown: &CancellationToken) -> TriggerResult<()> {
    let mut leader = engine.db().acquire().await?;
    let (got,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(LEADER_LOCK)
        .fetch_one(&mut *leader)
        .await?;
    if !got {
        return Ok(());
    }
    tracing::info!("this process is now the scheduler leader");
    loop {
        match tick(engine).await {
            Ok(0) => {}
            Ok(started) => tracing::info!(started, "started scheduled runs"),
            Err(e) => tracing::error!(error = %e, "scheduler tick failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                    .bind(LEADER_LOCK)
                    .execute(&mut *leader)
                    .await;
                return Ok(());
            }
            () = tokio::time::sleep(TICK) => {}
        }
        // Still connected, so still holding the lock.
        sqlx::query("SELECT 1").execute(&mut *leader).await?;
    }
}

/// A webhook trigger, found by its public id.
#[derive(Debug, Clone)]
pub struct Webhook {
    pub workflow_id: Uuid,
    pub org_id: Uuid,
    pub trigger_id: String,
    pub version: i32,
    pub config: WebhookTrigger,
    pub document: Json,
}

/// The webhook behind `public_id`, if it exists and its workflow is active
/// and published.
pub async fn find_webhook(engine: &Engine, public_id: &str) -> TriggerResult<Option<Webhook>> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let row: Option<(Uuid, Uuid, String, Json, i32)> = sqlx::query_as(
        "SELECT t.workflow_id, t.org_id, t.trigger_id, t.config, w.current_version \
           FROM workflow_triggers t JOIN workflows w ON w.id = t.workflow_id \
          WHERE t.public_id = $1 AND t.type = 'webhook' AND w.status = 'active' \
            AND w.current_version IS NOT NULL",
    )
    .bind(public_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((workflow_id, org_id, trigger_id, config, version)) = row else {
        tx.commit().await?;
        return Ok(None);
    };
    let document = huxley_workflows::version(&mut tx, workflow_id, version)
        .await?
        .document;
    tx.commit().await?;
    let Ok(Trigger::Webhook(config)) = serde_json::from_value(config) else {
        return Ok(None);
    };
    Ok(Some(Webhook {
        workflow_id,
        org_id,
        trigger_id,
        version,
        config,
        document,
    }))
}

/// Evaluates the webhook's auth expressions, which usually read a secret
/// organization variable (`{{ secrets.hook_secret }}`).
pub async fn resolve_auth(
    engine: &Engine,
    crypto: &huxley_crypto::Crypto,
    webhook: &Webhook,
) -> TriggerResult<Option<webhook::Resolved>> {
    let Some(auth) = webhook.config.auth.as_ref() else {
        return Ok(None);
    };
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let variables = huxley_variables::for_run(&mut tx, crypto, webhook.org_id).await?;
    tx.commit().await?;
    let scope = huxley_expr::Scope::new()
        .with("org", &json!({ "id": webhook.org_id }))
        .with("vars", &Json::Object(variables.vars))
        .with("env", &json!({}))
        .with_secrets(variables.secrets);
    let values = webhook::expressions(auth)
        .into_iter()
        .map(|e| {
            engine
                .expr()
                .eval_str(e, &scope)
                .ok()
                .and_then(|v| v.value.as_str().map(str::to_owned))
                .unwrap_or_default()
        })
        .collect();
    Ok(Some(webhook::resolve(auth, values)))
}

/// Starts a run for a verified webhook request.
pub async fn start(
    engine: &Engine,
    webhook: &Webhook,
    trigger: Json,
    inputs: Json,
) -> TriggerResult<Uuid> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let run = engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: webhook.org_id,
                workflow_id: Some(webhook.workflow_id),
                version: Some(webhook.version),
                document: None,
                trigger_type: "webhook".into(),
                trigger,
                inputs,
                actor_id: None,
            },
        )
        .await?;
    tx.commit().await?;
    Ok(run)
}
