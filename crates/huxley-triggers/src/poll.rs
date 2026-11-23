//! Integration poll triggers (design doc §6.3): at an interval, call a
//! list action and start a run for each new (or changed) item.
//!
//! - The scheduler leader claims one due poll at a time and moves its next
//!   check on before calling the API, so no transaction waits on HTTP.
//! - What a trigger has seen is kept in `poll_items` (key and content
//!   hash). Its first check (and the first after its configuration
//!   changes) only records a baseline: polling doesn't replay history.
//! - A failed check is kept as the trigger's `last_error` until one works.

use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, StartRun};
use huxley_hwf::{PollOn, PollTrigger, Trigger};
use ring::digest::{SHA256, digest};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::TriggerResult;

/// Most items one check looks at.
pub const MAX_ITEMS: usize = 500;
/// Most polls one pass checks.
const BATCH: usize = 20;

#[derive(sqlx::FromRow)]
struct DuePoll {
    id: Uuid,
    workflow_id: Uuid,
    org_id: Uuid,
    trigger_id: String,
    config: Json,
    last_run_at: Option<OffsetDateTime>,
    current_version: Option<i32>,
    active: bool,
}

/// The value at a dotted path (`value`, `data.items`); the whole value for
/// an empty path.
fn at<'a>(value: &'a Json, path: &str) -> Option<&'a Json> {
    path.split('.')
        .filter(|p| !p.is_empty())
        .try_fold(value, |v, part| v.get(part))
}

fn key_of(item: &Json, path: &str) -> Option<String> {
    match at(item, path)? {
        Json::String(s) if !s.is_empty() => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Checks due polls. Returns how many runs they started.
pub async fn poll_due(engine: &Engine) -> TriggerResult<usize> {
    let mut started = 0;
    for _ in 0..BATCH {
        let Some(poll) = claim(engine).await? else {
            break;
        };
        started += check(engine, &poll).await?;
    }
    Ok(started)
}

/// Takes the next due poll and moves its next check on.
async fn claim(engine: &Engine) -> TriggerResult<Option<DuePoll>> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let due: Option<DuePoll> = sqlx::query_as(
        "SELECT t.id, t.workflow_id, t.org_id, t.trigger_id, t.config, t.last_run_at, \
                w.current_version, w.status = 'active' AS active \
           FROM workflow_triggers t JOIN workflows w ON w.id = t.workflow_id \
          WHERE t.type = 'poll' AND t.next_run_at <= now() \
          ORDER BY t.next_run_at LIMIT 1 FOR UPDATE OF t SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(due) = due else {
        tx.commit().await?;
        return Ok(None);
    };
    let interval = match serde_json::from_value(due.config.clone()) {
        Ok(Trigger::Poll(p)) => huxley_hwf::duration::parse(&p.interval),
        _ => None,
    };
    // A broken configuration stops polling rather than spinning.
    let next = interval.map(|d| OffsetDateTime::now_utc() + d);
    sqlx::query("UPDATE workflow_triggers SET next_run_at = $2, updated_at = now() WHERE id = $1")
        .bind(due.id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(due))
}

async fn record_error(engine: &Engine, trigger: Uuid, message: &str) -> TriggerResult<()> {
    tracing::warn!(%trigger, "poll failed: {message}");
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    sqlx::query("UPDATE workflow_triggers SET last_error = $2 WHERE id = $1")
        .bind(trigger)
        .bind(message)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// One check of one poll. Returns how many runs it started.
async fn check(engine: &Engine, poll: &DuePoll) -> TriggerResult<usize> {
    let (Ok(Trigger::Poll(config)), Some(version), true) = (
        serde_json::from_value::<Trigger>(poll.config.clone()),
        poll.current_version,
        poll.active,
    ) else {
        return Ok(0);
    };
    let call = json!({
        "action": config.action,
        "connection": config.connection,
        "inputs": config.inputs,
    });
    let body = match engine.poll_action(poll.org_id, &call).await? {
        Ok(body) => body,
        Err(failure) => {
            let message = failure["message"].as_str().unwrap_or("the action failed");
            record_error(engine, poll.id, message).await?;
            return Ok(0);
        }
    };
    let path = config.items.as_deref().unwrap_or("");
    let Some(Json::Array(items)) = at(&body, path) else {
        let message = if path.is_empty() {
            "the response isn't a list: set `items` to where the list is".to_owned()
        } else {
            format!("the response has no list at {path:?}")
        };
        record_error(engine, poll.id, &message).await?;
        return Ok(0);
    };

    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let started = take_items(engine, &mut tx, poll, &config, version, items).await?;
    sqlx::query(
        "UPDATE workflow_triggers SET last_run_at = now(), last_error = NULL WHERE id = $1",
    )
    .bind(poll.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(started)
}

/// Records the items and starts runs for those that are new (or changed).
async fn take_items(
    engine: &Engine,
    tx: &mut PgConnection,
    poll: &DuePoll,
    config: &PollTrigger,
    version: i32,
    items: &[Json],
) -> TriggerResult<usize> {
    // The first check (or the first after a change) is the baseline.
    let baseline = poll.last_run_at.is_none();
    let id_path = config.item_id.as_deref().unwrap_or("id");
    // Checked on the first item that would start a run.
    let mut runnable: Option<Option<Option<Uuid>>> = None;
    let mut started = 0;
    for item in items.iter().take(MAX_ITEMS) {
        let Some(key) = key_of(item, id_path) else {
            continue;
        };
        let hash = digest(&SHA256, &serde_json::to_vec(item).unwrap_or_default())
            .as_ref()
            .to_vec();
        let old: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT hash FROM poll_items WHERE trigger_id = $1 AND item_key = $2",
        )
        .bind(poll.id)
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO poll_items (trigger_id, item_key, hash) VALUES ($1, $2, $3) \
             ON CONFLICT (trigger_id, item_key) DO UPDATE SET hash = EXCLUDED.hash, \
                    last_seen = now()",
        )
        .bind(poll.id)
        .bind(&key)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
        let change = match &old {
            None => "new",
            Some(old) if *old != hash && config.on == PollOn::NewOrChanged => "changed",
            Some(_) => continue,
        };
        if baseline {
            continue;
        }
        // Whether runs can start (inputs fit, run-as usable): checked once.
        if runnable.is_none() {
            runnable = Some(can_start(tx, poll, version).await?);
        }
        let Some(Some(actor)) = runnable else {
            continue;
        };
        engine
            .start_run(
                &mut *tx,
                StartRun {
                    org_id: poll.org_id,
                    workflow_id: Some(poll.workflow_id),
                    version: Some(version),
                    document: None,
                    trigger_type: "poll".into(),
                    trigger: json!({
                        "trigger_id": poll.trigger_id,
                        "change": change,
                        "item": item,
                    }),
                    inputs: json!({}),
                    actor_id: actor,
                },
            )
            .await?;
        started += 1;
    }
    Ok(started)
}

/// `Some(actor)` if runs can start: the workflow takes no required inputs
/// and its run-as account (if any) is usable. Logged when not.
async fn can_start(
    tx: &mut PgConnection,
    poll: &DuePoll,
    version: i32,
) -> TriggerResult<Option<Option<Uuid>>> {
    let document = huxley_workflows::version(&mut *tx, poll.workflow_id, version)
        .await?
        .document;
    if let Err(e) = huxley_workflows::check_inputs(&document, &json!({})) {
        tracing::warn!(
            workflow = %poll.workflow_id, error = %e,
            "poll runs skipped: the workflow needs inputs (polled items are in trigger.item)"
        );
        return Ok(None);
    }
    match huxley_workflows::trigger_actor(tx, poll.workflow_id).await? {
        Ok(actor) => Ok(Some(actor)),
        Err(reason) => {
            tracing::warn!(workflow = %poll.workflow_id, "poll runs skipped: {reason}");
            Ok(None)
        }
    }
}
