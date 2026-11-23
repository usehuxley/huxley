//! Triggers of the published version, kept in `workflow_triggers` for the
//! scheduler and the webhook endpoint.

use huxley_hwf::cron::{Cron, time_zone};
use huxley_hwf::{ScheduleTrigger, Trigger, Workflow};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use serde_json::Value as Json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkflowError, WorkflowResult};

/// A trigger as the app shows it.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TriggerView {
    pub trigger_id: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub kind: String,
    /// Webhooks: the path to POST to, `/hooks/{publicId}`.
    pub public_id: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_run_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_run_at: Option<OffsetDateTime>,
}

fn to_offset(at: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(at.as_nanosecond()).ok()
}

fn to_timestamp(at: OffsetDateTime) -> Option<Timestamp> {
    Timestamp::from_nanosecond(at.unix_timestamp_nanos()).ok()
}

/// The time zone a schedule runs in: its own, else the org's setting, else UTC.
pub async fn schedule_zone(
    conn: &mut PgConnection,
    org_id: Uuid,
    schedule: &ScheduleTrigger,
) -> WorkflowResult<TimeZone> {
    if let Some(tz) = schedule.timezone.as_deref().and_then(time_zone) {
        return Ok(tz);
    }
    let org_zone = huxley_orgs::settings::value(conn, org_id, huxley_orgs::settings::TIMEZONE)
        .await
        .map_err(|e| WorkflowError::InvalidDocument(e.to_string()))?;
    Ok(org_zone
        .as_deref()
        .and_then(time_zone)
        .unwrap_or(TimeZone::UTC))
}

/// When a schedule next fires after `after`.
pub fn next_run(
    schedule: &ScheduleTrigger,
    zone: &TimeZone,
    after: OffsetDateTime,
) -> Option<OffsetDateTime> {
    if let Some(cron) = schedule.cron.as_deref() {
        let cron = Cron::parse(cron).ok()?;
        to_offset(cron.next_after(to_timestamp(after)?, zone)?)
    } else {
        let every = schedule
            .interval
            .as_deref()
            .and_then(huxley_hwf::duration::parse)?;
        Some(after + every)
    }
}

fn public_id() -> String {
    let mut bytes = [0u8; 18];
    getrandom::fill(&mut bytes).expect("the operating system RNG is available");
    bytes.iter().fold(String::from("wh_"), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Replaces the workflow's triggers with those of `document` (a published
/// version). A changed schedule starts counting again from now.
pub async fn sync(
    conn: &mut PgConnection,
    workflow_id: Uuid,
    org_id: Uuid,
    document: &Json,
) -> WorkflowResult<()> {
    let workflow: Workflow = serde_json::from_value(document.clone())
        .map_err(|e| WorkflowError::InvalidDocument(e.to_string()))?;
    let now = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for trigger in &workflow.spec.triggers {
        let trigger_id = trigger.id().unwrap_or(trigger.type_name()).to_owned();
        let config = serde_json::to_value(trigger).unwrap_or(Json::Null);
        let next_run_at = match trigger {
            Trigger::Schedule(schedule) => {
                let zone = schedule_zone(conn, org_id, schedule).await?;
                next_run(schedule, &zone, now)
            }
            _ => None,
        };
        let public = matches!(trigger, Trigger::Webhook(_)).then(public_id);
        sqlx::query(
            "INSERT INTO workflow_triggers (id, workflow_id, org_id, trigger_id, type, config, \
                    public_id, next_run_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (workflow_id, trigger_id) DO UPDATE SET \
                    type = EXCLUDED.type, \
                    config = EXCLUDED.config, \
                    public_id = CASE WHEN EXCLUDED.type = 'webhook' \
                                     THEN COALESCE(workflow_triggers.public_id, EXCLUDED.public_id) END, \
                    next_run_at = CASE \
                        WHEN workflow_triggers.config IS DISTINCT FROM EXCLUDED.config \
                          OR workflow_triggers.next_run_at IS NULL \
                        THEN EXCLUDED.next_run_at \
                        ELSE workflow_triggers.next_run_at END, \
                    updated_at = now()",
        )
        .bind(huxley_core::new_id())
        .bind(workflow_id)
        .bind(org_id)
        .bind(&trigger_id)
        .bind(trigger.type_name())
        .bind(&config)
        .bind(public)
        .bind(next_run_at)
        .execute(&mut *conn)
        .await?;
        ids.push(trigger_id);
    }
    sqlx::query(
        "DELETE FROM workflow_triggers WHERE workflow_id = $1 AND NOT trigger_id = ANY($2)",
    )
    .bind(workflow_id)
    .bind(&ids)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the workflow's triggers (when it is archived).
pub async fn clear(conn: &mut PgConnection, workflow_id: Uuid) -> WorkflowResult<()> {
    sqlx::query("DELETE FROM workflow_triggers WHERE workflow_id = $1")
        .bind(workflow_id)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn list(conn: &mut PgConnection, workflow_id: Uuid) -> WorkflowResult<Vec<TriggerView>> {
    Ok(sqlx::query_as(
        "SELECT trigger_id, type, public_id, next_run_at, last_run_at FROM workflow_triggers \
          WHERE workflow_id = $1 ORDER BY trigger_id",
    )
    .bind(workflow_id)
    .fetch_all(conn)
    .await?)
}
