//! Event triggers: domain events from the outbox start workflow runs
//! (design doc §6.5).
//!
//! An event matches a published workflow's `event` trigger when the trigger
//! names its type (or a prefix wildcard like `org.*`), the workflow belongs
//! to the event's org or one of its ancestors, and the trigger's `filter`
//! (if any) is true. Only events after the trigger was first published
//! count: publishing doesn't replay history. The run happens in the event's org, so organization
//! variables resolve for it. Runs are started, and the event marked
//! published, in one transaction: each event is dispatched exactly once.

use std::time::Duration;

use huxley_crypto::Crypto;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, StartRun};
use huxley_hwf::Trigger;
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::TriggerResult;

const BATCH: i64 = 100;
const POLL: Duration = Duration::from_secs(1);

#[derive(sqlx::FromRow)]
struct Outboxed {
    id: Uuid,
    occurred_at: OffsetDateTime,
    org_id: Option<Uuid>,
    event_type: String,
    subject_type: String,
    subject_id: Uuid,
    payload: Json,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    workflow_id: Uuid,
    workflow_org: Uuid,
    trigger_id: String,
    config: Json,
    current_version: i32,
}

/// Whether a trigger's `event` pattern matches an event type.
pub fn matches(pattern: &str, event_type: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => event_type
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.')),
        None => pattern == event_type,
    }
}

/// Dispatches one batch of unpublished events. Returns how many runs started.
pub async fn dispatch(engine: &Engine, crypto: &Crypto) -> TriggerResult<usize> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let events: Vec<Outboxed> = sqlx::query_as(
        "SELECT id, occurred_at, org_id, event_type, subject_type, subject_id, payload \
           FROM events_outbox WHERE published_at IS NULL \
          ORDER BY occurred_at LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(BATCH)
    .fetch_all(&mut *tx)
    .await?;
    let mut started = 0;
    for event in &events {
        started += dispatch_one(engine, crypto, &mut tx, event).await?;
    }
    if !events.is_empty() {
        let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
        sqlx::query("UPDATE events_outbox SET published_at = now() WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(started)
}

async fn dispatch_one(
    engine: &Engine,
    crypto: &Crypto,
    conn: &mut PgConnection,
    event: &Outboxed,
) -> TriggerResult<usize> {
    // Workflows of the event's org and its ancestors (or of the root, for
    // events that belong to no org) with an event trigger.
    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT t.workflow_id, w.org_id AS workflow_org, t.trigger_id, t.config, \
                w.current_version \
           FROM workflow_triggers t \
           JOIN workflows w ON w.id = t.workflow_id \
           JOIN organizations o ON o.id = w.org_id \
          WHERE t.type = 'event' AND w.status = 'active' AND w.current_version IS NOT NULL \
            AND t.created_at <= $2 \
            AND CASE WHEN $1::uuid IS NULL THEN o.parent_id IS NULL \
                     ELSE o.path @> (SELECT path FROM organizations WHERE id = $1) END",
    )
    .bind(event.org_id)
    .bind(event.occurred_at)
    .fetch_all(&mut *conn)
    .await?;
    let candidates: Vec<(Candidate, Option<String>)> = candidates
        .into_iter()
        .filter_map(
            |c| match serde_json::from_value::<Trigger>(c.config.clone()) {
                Ok(Trigger::Event(t)) if matches(&t.event, &event.event_type) => {
                    Some((c, t.filter))
                }
                _ => None,
            },
        )
        .collect();
    if candidates.is_empty() {
        return Ok(0);
    }

    let event_json = json!({
        "id": event.id,
        "type": event.event_type,
        "org_id": event.org_id,
        "subject_type": event.subject_type,
        "subject_id": event.subject_id,
        "payload": event.payload,
        "occurred_at": event.occurred_at.format(&Rfc3339).unwrap_or_default(),
    });
    let mut started = 0;
    for (candidate, filter) in candidates {
        let run_org = event.org_id.unwrap_or(candidate.workflow_org);
        if let Some(filter) = filter
            && !passes(
                engine,
                crypto,
                conn,
                &candidate,
                run_org,
                &filter,
                &event_json,
            )
            .await?
        {
            continue;
        }
        let inputs = if event.payload.is_object() {
            event.payload.clone()
        } else {
            json!({})
        };
        let Some(actor) = runnable(&mut *conn, &candidate, &inputs).await? else {
            continue;
        };
        engine
            .start_run(
                &mut *conn,
                StartRun {
                    org_id: run_org,
                    workflow_id: Some(candidate.workflow_id),
                    version: Some(candidate.current_version),
                    document: None,
                    trigger_type: "event".into(),
                    trigger: {
                        let mut trigger = event_json.clone();
                        trigger["trigger_id"] = json!(candidate.trigger_id);
                        trigger
                    },
                    inputs,
                    actor_id: actor,
                },
            )
            .await?;
        started += 1;
    }
    Ok(started)
}

/// Whether a matching workflow can run for an event: its inputs fit the
/// schema and its run-as account (the run's actor) is usable. `None` (with
/// a warning logged) skips it.
async fn runnable(
    conn: &mut PgConnection,
    candidate: &Candidate,
    inputs: &Json,
) -> TriggerResult<Option<Option<Uuid>>> {
    let document =
        huxley_workflows::version(&mut *conn, candidate.workflow_id, candidate.current_version)
            .await?
            .document;
    if let Err(e) = huxley_workflows::check_inputs(&document, inputs) {
        tracing::warn!(
            workflow = %candidate.workflow_id, error = %e,
            "event run skipped: the event doesn't match the workflow's input schema"
        );
        return Ok(None);
    }
    match huxley_workflows::trigger_actor(conn, candidate.workflow_id).await? {
        Ok(actor) => Ok(Some(actor)),
        Err(reason) => {
            tracing::warn!(workflow = %candidate.workflow_id, "event run skipped: {reason}");
            Ok(None)
        }
    }
}

/// Evaluates a trigger's filter for an event; a failing filter is logged
/// and counts as false.
async fn passes(
    engine: &Engine,
    crypto: &Crypto,
    conn: &mut PgConnection,
    candidate: &Candidate,
    run_org: Uuid,
    filter: &str,
    event: &Json,
) -> TriggerResult<bool> {
    let variables = huxley_variables::for_run(conn, crypto, run_org).await?;
    let scope = huxley_expr::Scope::new()
        .with("event", event)
        .with("org", &json!({ "id": run_org }))
        .with("vars", &Json::Object(variables.vars))
        .with("env", &json!({}))
        .with_secrets(variables.secrets);
    match engine.expr().eval_condition(filter, &scope) {
        Ok(yes) => Ok(yes),
        Err(e) => {
            tracing::warn!(
                workflow = %candidate.workflow_id, trigger = %candidate.trigger_id,
                error = %e, "event trigger filter failed; skipping"
            );
            Ok(false)
        }
    }
}

/// Dispatches events until `shutdown`. Any number of processes may run it.
pub async fn run_event_dispatcher(engine: Engine, crypto: Crypto, shutdown: CancellationToken) {
    loop {
        match dispatch(&engine, &crypto).await {
            Ok(0) => {}
            Ok(started) => tracing::info!(started, "started runs from events"),
            Err(e) => tracing::error!(error = %e, "event dispatch failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn patterns() {
        assert!(matches("org.created", "org.created"));
        assert!(!matches("org.created", "org.updated"));
        assert!(matches("org.*", "org.created"));
        assert!(matches("org.*", "org.custom_field.set"));
        assert!(!matches("org.*", "organization.created"));
        assert!(!matches("org.*", "org"));
    }
}
