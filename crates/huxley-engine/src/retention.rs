//! Data retention (ADR-085): a daily sweep removes finished workflow runs
//! older than the retention (with their spilled payloads in object storage),
//! published domain events, read notifications and AI call records past
//! theirs, then drops run partitions it emptied. The audit log isn't swept.

use std::time::Duration;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Engine, EngineError, EngineResult};

/// How long things are kept.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retention {
    /// Finished workflow runs, in months (1 to 120).
    pub run_months: i32,
    /// Published domain events, in days.
    pub event_days: i32,
    /// Read notifications, in days.
    pub notification_days: i32,
    /// AI call records (usage and cost), in days.
    pub ai_call_days: i32,
}

/// What a sweep removed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Swept {
    pub runs: u64,
    pub payload_objects: u64,
    pub partitions: i32,
    pub events: u64,
    pub notifications: u64,
    pub ai_calls: u64,
}

/// The settings, with when the last sweep ran and what it removed.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionState {
    #[serde(flatten)]
    pub retention: Retention,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_swept_at: Option<OffsetDateTime>,
    pub last_sweep: Option<Swept>,
}

pub async fn get(conn: &mut PgConnection) -> EngineResult<RetentionState> {
    let (run_months, event_days, notification_days, ai_call_days, last_swept_at, last_sweep): (
        i32,
        i32,
        i32,
        i32,
        Option<OffsetDateTime>,
        Option<serde_json::Value>,
    ) = sqlx::query_as(
        "SELECT run_months, event_days, notification_days, ai_call_days, last_swept_at, last_sweep \
           FROM retention_settings WHERE id",
    )
    .fetch_one(conn)
    .await?;
    Ok(RetentionState {
        retention: Retention {
            run_months,
            event_days,
            notification_days,
            ai_call_days,
        },
        last_swept_at,
        last_sweep: last_sweep.and_then(|v| serde_json::from_value(v).ok()),
    })
}

fn check(r: &Retention) -> EngineResult<()> {
    let bad = |field: &str, range: &str| Err(EngineError::Invalid(format!("{field}: {range}")));
    if !(1..=120).contains(&r.run_months) {
        return bad("runMonths", "1 to 120");
    }
    for (field, v) in [
        ("eventDays", r.event_days),
        ("notificationDays", r.notification_days),
        ("aiCallDays", r.ai_call_days),
    ] {
        if !(1..=3650).contains(&v) {
            return bad(field, "1 to 3650");
        }
    }
    Ok(())
}

pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    r: &Retention,
) -> EngineResult<RetentionState> {
    check(r)?;
    sqlx::query(
        "UPDATE retention_settings SET run_months = $1, event_days = $2, notification_days = $3, ai_call_days = $4, \
            updated_at = now() WHERE id",
    )
    .bind(r.run_months)
    .bind(r.event_days)
    .bind(r.notification_days)
    .bind(r.ai_call_days)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("retention.changed").details(json!(r)),
    )
    .await?;
    get(conn).await
}

/// Runs removed per batch.
const BATCH: i64 = 500;

impl Engine {
    /// Sweeps what's past its retention. Run in system scope.
    pub async fn sweep_retention(&self) -> EngineResult<Swept> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        let r = get(&mut tx).await?.retention;
        tx.commit().await?;
        let mut out = Swept::default();
        let cutoff = OffsetDateTime::now_utc()
            - Duration::from_secs(u64::try_from(r.run_months).unwrap_or(12) * 30 * 86_400);
        loop {
            let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
            let batch: Vec<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT org_id, id FROM runs WHERE id < huxley_uuid7_floor($1) \
                   AND status IN ('succeeded', 'failed', 'cancelled') ORDER BY id LIMIT $2",
            )
            .bind(cutoff)
            .bind(BATCH)
            .fetch_all(&mut *tx)
            .await?;
            if batch.is_empty() {
                tx.commit().await?;
                break;
            }
            if let Some(storage) = self.payloads.storage() {
                for (org, run) in &batch {
                    let removed = storage
                        .delete_prefix(&format!("workflow-payloads/{org}/{run}/"))
                        .await
                        .map_err(|e| EngineError::Storage(e.to_string()))?;
                    out.payload_objects += removed as u64;
                }
            }
            let ids: Vec<Uuid> = batch.iter().map(|(_, id)| *id).collect();
            out.runs += sqlx::query("DELETE FROM runs WHERE id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            tx.commit().await?;
        }
        let mut tx = begin_scoped(self.db(), &OrgScope::System).await?;
        out.partitions =
            sqlx::query_scalar("SELECT huxley_drop_empty_run_partitions(huxley_uuid7_floor($1))")
                .bind(cutoff)
                .fetch_one(&mut *tx)
                .await?;
        out.events = sqlx::query(
            "DELETE FROM events_outbox WHERE published_at < now() - make_interval(days => $1)",
        )
        .bind(r.event_days)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        out.notifications = sqlx::query(
            "DELETE FROM notifications WHERE read_at < now() - make_interval(days => $1)",
        )
        .bind(r.notification_days)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        out.ai_calls = sqlx::query(
            "DELETE FROM ai_calls WHERE created_at < now() - make_interval(days => $1)",
        )
        .bind(r.ai_call_days)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Rate limit counters past any window (ADR-090).
        sqlx::query("DELETE FROM rate_limit_hits WHERE window_start < now() - interval '1 hour'")
            .execute(&mut *tx)
            .await?;
        // Signed webhook deliveries past their replay window.
        sqlx::query("DELETE FROM webhook_deliveries WHERE expires_at < now()")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE retention_settings SET last_swept_at = now(), last_sweep = $1 WHERE id",
        )
        .bind(json!(out))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(out)
    }
}

/// Sweeps a few minutes after start, then daily, until `shutdown`.
pub async fn run_sweeper(engine: Engine, shutdown: CancellationToken) {
    let mut wait = Duration::from_secs(300);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        match engine.sweep_retention().await {
            Ok(swept) => tracing::info!(?swept, "retention sweep"),
            Err(e) => tracing::error!(error = %e, "retention sweep failed"),
        }
        wait = Duration::from_secs(86_400);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_checked() {
        let ok = Retention {
            run_months: 12,
            event_days: 30,
            notification_days: 180,
            ai_call_days: 365,
        };
        assert!(check(&ok).is_ok());
        assert!(
            check(&Retention {
                run_months: 0,
                ..ok.clone()
            })
            .is_err()
        );
        assert!(
            check(&Retention {
                ai_call_days: 4000,
                ..ok
            })
            .is_err()
        );
    }
}
