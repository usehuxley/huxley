//! People's availability (design doc §8.15): working schedules, time off
//! (requested, then approved or declined), and whether someone is
//! available at a moment.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_core::calendar::Calendar;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkforceError, WorkforceResult, text};

pub const KINDS: &[&str] = &["vacation", "sick", "training", "other"];

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    /// An IANA time zone, e.g. `Europe/London`.
    pub timezone: String,
    /// `{"mon": [["09:00", "17:00"]], …}`; days left out aren't worked.
    #[schema(value_type = std::collections::HashMap<String, Vec<Vec<String>>>)]
    pub hours: Json,
}

/// Someone's schedule, if they have one.
pub async fn schedule(conn: &mut PgConnection, user_id: Uuid) -> WorkforceResult<Option<Schedule>> {
    let row: Option<(String, Json)> =
        sqlx::query_as("SELECT timezone, hours FROM wf_schedules WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(timezone, hours)| Schedule { timezone, hours }))
}

/// Sets someone's schedule (`None` removes it: no hours limit).
pub async fn set_schedule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    input: Option<&Schedule>,
) -> WorkforceResult<Option<Schedule>> {
    match input {
        None => {
            sqlx::query("DELETE FROM wf_schedules WHERE user_id = $1")
                .bind(user_id)
                .execute(&mut *conn)
                .await?;
        }
        Some(s) => {
            let timezone = s.timezone.trim();
            if jiff::tz::TimeZone::get(timezone).is_err() {
                return Err(WorkforceError::invalid(
                    "timezone",
                    format!("{timezone:?} isn't a time zone"),
                ));
            }
            let calendar = Calendar::weekly(timezone, &s.hours).ok_or_else(|| {
                WorkforceError::invalid("hours", "some working hours, as HH:MM ranges")
            })?;
            sqlx::query(
                "INSERT INTO wf_schedules (user_id, timezone, hours) VALUES ($1, $2, $3) \
                 ON CONFLICT (user_id) DO UPDATE SET timezone = EXCLUDED.timezone, hours = EXCLUDED.hours, \
                   updated_at = now()",
            )
            .bind(user_id)
            .bind(timezone)
            .bind(calendar.hours())
            .execute(&mut *conn)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => WorkforceError::NotFound,
                _ => WorkforceError::Sqlx(e),
            })?;
        }
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("person.schedule_set")
            .target("user", user_id)
            .details(json!({ "schedule": input })),
    )
    .await?;
    schedule(conn, user_id).await
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TimeOff {
    pub id: Uuid,
    pub user_id: Uuid,
    pub person: String,
    /// `YYYY-MM-DD`.
    pub starts_on: String,
    /// `YYYY-MM-DD`, inclusive.
    pub ends_on: String,
    /// `vacation`, `sick`, `training` or `other`.
    pub kind: String,
    pub note: String,
    /// `requested`, `approved`, `declined` or `cancelled`.
    pub status: String,
    pub decided_by: Option<String>,
    pub decision_note: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeOffRequest {
    /// `YYYY-MM-DD`.
    pub starts_on: String,
    /// `YYYY-MM-DD`, inclusive.
    pub ends_on: String,
    /// `vacation`, `sick`, `training` or `other`.
    pub kind: String,
    #[serde(default)]
    pub note: Option<String>,
}

const SELECT: &str = "SELECT t.id, t.user_id, u.display_name AS person, to_char(t.starts_on, 'YYYY-MM-DD') AS starts_on, \
    to_char(t.ends_on, 'YYYY-MM-DD') AS ends_on, t.kind, t.note, t.status, d.display_name AS decided_by, \
    t.decision_note FROM wf_time_off t JOIN users u ON u.id = t.user_id LEFT JOIN users d ON d.id = t.decided_by";

pub async fn time_off_one(conn: &mut PgConnection, id: Uuid) -> WorkforceResult<TimeOff> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(WorkforceError::NotFound)
}

/// Time off: someone's (or everyone's), that hasn't ended before `from`
/// (default: today), optionally only in one status. Soonest first.
pub async fn time_off(
    conn: &mut PgConnection,
    user_id: Option<Uuid>,
    status: Option<&str>,
) -> WorkforceResult<Vec<TimeOff>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR t.user_id = $1) AND ($2::text IS NULL OR t.status = $2) \
           AND t.ends_on >= current_date - 30 ORDER BY t.starts_on, t.created_at"
    )))
    .bind(user_id)
    .bind(status)
    .fetch_all(conn)
    .await?)
}

fn date(field: &str, value: &str) -> WorkforceResult<String> {
    let v = value.trim();
    let ok = v.len() == 10
        && v.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if ok {
        Ok(v.to_owned())
    } else {
        Err(WorkforceError::invalid(field, "a date, as YYYY-MM-DD"))
    }
}

async fn announce(conn: &mut PgConnection, event: &str, t: &TimeOff) -> WorkforceResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: None,
            subject_type: "user",
            subject_id: t.user_id,
            payload: json!({
                "time_off_id": t.id,
                "user_id": t.user_id,
                "person": t.person,
                "starts_on": t.starts_on,
                "ends_on": t.ends_on,
                "kind": t.kind,
                "status": t.status,
            }),
        },
    )
    .await?;
    Ok(())
}

/// Requests time off for someone. `approved_by`: a manager booking it, who
/// approves it at once.
pub async fn request(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    input: &TimeOffRequest,
    approved_by: Option<Uuid>,
) -> WorkforceResult<TimeOff> {
    let starts = date("startsOn", &input.starts_on)?;
    let ends = date("endsOn", &input.ends_on)?;
    if !KINDS.contains(&input.kind.as_str()) {
        return Err(WorkforceError::invalid("kind", KINDS.join(", ")));
    }
    let note = text("note", input.note.as_deref(), 2000)?.unwrap_or_default();
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO wf_time_off (id, user_id, starts_on, ends_on, kind, note, status, decided_by, decided_at) \
         VALUES ($1, $2, $3::date, $4::date, $5, $6, \
                 CASE WHEN $7::uuid IS NULL THEN 'requested' ELSE 'approved' END, $7, \
                 CASE WHEN $7::uuid IS NULL THEN NULL ELSE now() END)",
    )
    .bind(id)
    .bind(user_id)
    .bind(&starts)
    .bind(&ends)
    .bind(&input.kind)
    .bind(&note)
    .bind(approved_by)
    .execute(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if matches!(d.code().as_deref(), Some("22007" | "22008")) => {
            WorkforceError::invalid("startsOn", "not a real date")
        }
        sqlx::Error::Database(d) if d.is_check_violation() => {
            WorkforceError::invalid("endsOn", "on or after the start, and within a year of it")
        }
        sqlx::Error::Database(d) if d.is_foreign_key_violation() => WorkforceError::NotFound,
        _ => WorkforceError::Sqlx(e),
    })?;
    let t = time_off_one(conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("time_off.requested")
            .target("user", user_id)
            .details(
                json!({ "timeOff": id, "startsOn": starts, "endsOn": ends, "kind": input.kind }),
            ),
    )
    .await?;
    announce(
        conn,
        if approved_by.is_some() {
            "time_off.approved"
        } else {
            "time_off.requested"
        },
        &t,
    )
    .await?;
    Ok(t)
}

/// Approves or declines requested time off (`by` is the manager).
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    approve: bool,
    by: Uuid,
    note: Option<&str>,
) -> WorkforceResult<TimeOff> {
    let note = text("note", note, 2000)?.unwrap_or_default();
    let before = time_off_one(conn, id).await?;
    if before.user_id == by {
        return Err(WorkforceError::invalid(
            "approve",
            "someone else decides your time off",
        ));
    }
    if before.status != "requested" {
        return Err(WorkforceError::invalid(
            "approve",
            format!("it's already {}", before.status),
        ));
    }
    sqlx::query(
        "UPDATE wf_time_off SET status = $2, decided_by = $3, decided_at = now(), decision_note = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "declined" })
    .bind(by)
    .bind(&note)
    .execute(&mut *conn)
    .await?;
    let t = time_off_one(conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new(if approve {
            "time_off.approved"
        } else {
            "time_off.declined"
        })
        .target("user", t.user_id)
        .details(json!({ "timeOff": id })),
    )
    .await?;
    announce(
        conn,
        if approve {
            "time_off.approved"
        } else {
            "time_off.declined"
        },
        &t,
    )
    .await?;
    Ok(t)
}

/// Cancels time off that hasn't ended.
pub async fn cancel(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> WorkforceResult<TimeOff> {
    let rows = sqlx::query(
        "UPDATE wf_time_off SET status = 'cancelled' \
          WHERE id = $1 AND status IN ('requested', 'approved') AND ends_on >= current_date",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let t = time_off_one(conn, id).await?;
    if rows == 0 {
        return Err(WorkforceError::invalid(
            "status",
            "only time off that hasn't ended can be cancelled",
        ));
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("time_off.cancelled")
            .target("user", t.user_id)
            .details(json!({ "timeOff": id })),
    )
    .await?;
    announce(conn, "time_off.cancelled", &t).await?;
    Ok(t)
}

/// Why someone isn't available at a moment, if they aren't: `away` (on
/// approved time off that day, in their time zone) or `off_hours`
/// (outside their schedule). People without a schedule have no hours limit.
pub async fn unavailable(
    conn: &mut PgConnection,
    people: &[Uuid],
    at: i64,
) -> WorkforceResult<BTreeMap<Uuid, &'static str>> {
    let schedules: Vec<(Uuid, String, Json)> =
        sqlx::query_as("SELECT user_id, timezone, hours FROM wf_schedules WHERE user_id = ANY($1)")
            .bind(people)
            .fetch_all(&mut *conn)
            .await?;
    // Away: approved time off covering the day it is for them.
    let away: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT t.user_id FROM wf_time_off t \
           LEFT JOIN wf_schedules s ON s.user_id = t.user_id \
          WHERE t.user_id = ANY($1) AND t.status = 'approved' \
            AND (to_timestamp($2) AT TIME ZONE coalesce(s.timezone, 'UTC'))::date BETWEEN t.starts_on AND t.ends_on",
    )
    .bind(people)
    .bind(at)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = BTreeMap::new();
    for (user, tz, hours) in &schedules {
        if Calendar::weekly(tz, hours).is_some_and(|c| !c.is_open(at)) {
            out.insert(*user, "off_hours");
        }
    }
    for user in away {
        out.insert(user, "away");
    }
    Ok(out)
}
