//! Time entries (design doc §8.2, §8.15): time logged on tickets, or as
//! non-ticket time, with a work type. Each person logs and changes their
//! own. Ticket time sits in the ticket's organization; non-ticket time has
//! none and is read in system scope, by its owner.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::Ticket;
use crate::{DeskError, DeskResult, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkType {
    pub id: Uuid,
    pub name: String,
    /// Entries of this type are billable unless said otherwise.
    pub billable: bool,
    /// Offered on tickets (else only for non-ticket time).
    pub on_tickets: bool,
    pub archived: bool,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkTypeInput {
    pub name: String,
    #[serde(default = "yes")]
    pub billable: bool,
    #[serde(default = "yes")]
    pub on_tickets: bool,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

fn yes() -> bool {
    true
}

const WORK_TYPES: &str = "SELECT id, name, billable, on_tickets, archived_at IS NOT NULL AS archived, version \
    FROM sd_work_types";

/// Work types in order; archived ones only when asked.
pub async fn work_types(conn: &mut PgConnection, archived: bool) -> DeskResult<Vec<WorkType>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{WORK_TYPES} WHERE $1 OR archived_at IS NULL ORDER BY archived_at IS NOT NULL, position, lower(name)"
    )))
    .bind(archived)
    .fetch_all(conn)
    .await?)
}

async fn work_type(conn: &mut PgConnection, id: Uuid) -> DeskResult<WorkType> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{WORK_TYPES} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

fn work_type_taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_work_types_name") => {
            DeskError::invalid("name", "another work type has this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

pub async fn create_work_type(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &WorkTypeInput,
) -> DeskResult<WorkType> {
    let name = text("name", Some(&input.name), 60)?
        .ok_or_else(|| DeskError::invalid("name", "a work type needs a name"))?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_work_types (id, name, billable, on_tickets, position) \
         VALUES ($1, $2, $3, $4, (SELECT coalesce(max(position), 0) + 1 FROM sd_work_types))",
    )
    .bind(id)
    .bind(&name)
    .bind(input.billable)
    .bind(input.on_tickets)
    .execute(&mut *conn)
    .await
    .map_err(work_type_taken)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("work_type.created")
            .target("work_type", id)
            .details(json!({ "name": name })),
    )
    .await?;
    work_type(conn, id).await
}

pub async fn update_work_type(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &WorkTypeInput,
) -> DeskResult<WorkType> {
    let name = text("name", Some(&input.name), 60)?
        .ok_or_else(|| DeskError::invalid("name", "a work type needs a name"))?;
    let rows = sqlx::query(
        "UPDATE sd_work_types SET name = $3, billable = $4, on_tickets = $5, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&name)
    .bind(input.billable)
    .bind(input.on_tickets)
    .execute(&mut *conn)
    .await
    .map_err(work_type_taken)?
    .rows_affected();
    if rows == 0 {
        work_type(conn, id).await?;
        return Err(DeskError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("work_type.updated")
            .target("work_type", id)
            .details(
                json!({ "name": name, "billable": input.billable, "onTickets": input.on_tickets }),
            ),
    )
    .await?;
    work_type(conn, id).await
}

/// Archives a work type: it's no longer offered; its entries keep it.
pub async fn archive_work_type(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> DeskResult<()> {
    let found = work_type(conn, id).await?;
    sqlx::query(
        "UPDATE sd_work_types SET archived_at = coalesce(archived_at, now()) WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("work_type.archived")
            .target("work_type", id)
            .details(json!({ "name": found.name })),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntry {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    pub ticket_id: Option<Uuid>,
    pub ticket_reference: Option<String>,
    pub ticket_title: Option<String>,
    pub work_type_id: Uuid,
    pub work_type_name: String,
    /// `YYYY-MM-DD`.
    pub worked_on: String,
    pub minutes: i32,
    pub billable: bool,
    pub notes: String,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeEntryInput {
    pub work_type_id: Uuid,
    /// `YYYY-MM-DD` (default today).
    #[serde(default)]
    pub worked_on: Option<String>,
    /// 1 to 1440.
    pub minutes: i32,
    /// Default: the work type's.
    #[serde(default)]
    pub billable: Option<bool>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const ENTRIES: &str = "SELECT e.id, e.user_id, u.display_name AS user_name, e.ticket_id, \
    t.number_prefix || '-' || t.number AS ticket_reference, t.title AS ticket_title, \
    e.work_type_id, w.name AS work_type_name, to_char(e.worked_on, 'YYYY-MM-DD') AS worked_on, \
    e.minutes, e.billable, e.notes, e.version \
    FROM sd_time_entries e JOIN users u ON u.id = e.user_id JOIN sd_work_types w ON w.id = e.work_type_id \
    LEFT JOIN sd_tickets t ON t.id = e.ticket_id";

pub async fn entry(conn: &mut PgConnection, id: Uuid) -> DeskResult<TimeEntry> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRIES} WHERE e.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// A ticket's time, newest first.
pub async fn for_ticket(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Vec<TimeEntry>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ENTRIES} WHERE e.ticket_id = $1 ORDER BY e.worked_on DESC, e.created_at DESC"
    )))
    .bind(ticket)
    .fetch_all(conn)
    .await?)
}

/// Someone's time from `from` to `to` (inclusive), in order. System scope.
pub async fn for_person(
    conn: &mut PgConnection,
    user: Uuid,
    from: &str,
    to: &str,
) -> DeskResult<Vec<TimeEntry>> {
    let (start, end) = (day("from", from)?, day("to", to)?);
    let span = end.since(start).map_or(-1, |s| s.get_days());
    if !(0..=62).contains(&span) {
        return Err(DeskError::invalid(
            "to",
            "on or after from, and at most 62 days later",
        ));
    }
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ENTRIES} WHERE e.user_id = $1 AND e.worked_on BETWEEN $2::date AND $3::date \
          ORDER BY e.worked_on, e.created_at"
    )))
    .bind(user)
    .bind(start.to_string())
    .bind(end.to_string())
    .fetch_all(conn)
    .await?)
}

fn day(field: &str, value: &str) -> DeskResult<jiff::civil::Date> {
    value
        .trim()
        .parse::<jiff::civil::Date>()
        .map_err(|_| DeskError::invalid(field, "a date, as YYYY-MM-DD"))
}

/// Refuses changes to `user`'s time on a day their timesheet is submitted
/// or approved (a database trigger backs this up).
async fn unlocked(conn: &mut PgConnection, user: Uuid, worked_on: &str) -> DeskResult<()> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM wf_timesheets WHERE user_id = $1 AND $2::date BETWEEN period_start AND period_end \
            AND status IN ('submitted', 'approved')",
    )
    .bind(user)
    .bind(worked_on)
    .fetch_optional(conn)
    .await?;
    match status {
        Some(status) => Err(DeskError::invalid(
            "workedOn",
            format!("the timesheet for {worked_on} is {status}; it has to be reopened first"),
        )),
        None => Ok(()),
    }
}

struct Checked {
    worked_on: String,
    billable: bool,
    notes: String,
}

async fn check(
    conn: &mut PgConnection,
    input: &TimeEntryInput,
    on_ticket: bool,
) -> DeskResult<Checked> {
    let kind = work_type(conn, input.work_type_id)
        .await
        .map_err(|_| DeskError::invalid("workTypeId", "there's no such work type"))?;
    if kind.archived {
        return Err(DeskError::invalid(
            "workTypeId",
            "that work type is archived",
        ));
    }
    if on_ticket && !kind.on_tickets {
        return Err(DeskError::invalid(
            "workTypeId",
            format!("{} isn't for tickets", kind.name),
        ));
    }
    if !(1..=1440).contains(&input.minutes) {
        return Err(DeskError::invalid("minutes", "1 minute to 24 hours"));
    }
    let today = jiff::Zoned::now().date();
    let worked_on = match input
        .worked_on
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        None => today,
        Some(d) => d
            .parse::<jiff::civil::Date>()
            .map_err(|_| DeskError::invalid("workedOn", "a date, as YYYY-MM-DD"))?,
    };
    let span = today.since(worked_on).map_or(0, |s| s.get_days());
    if !(-1..=366).contains(&span) {
        return Err(DeskError::invalid(
            "workedOn",
            "within the past year, and not ahead",
        ));
    }
    Ok(Checked {
        worked_on: worked_on.to_string(),
        billable: input.billable.unwrap_or(kind.billable),
        notes: text("notes", input.notes.as_deref(), 4000)?.unwrap_or_default(),
    })
}

/// Logs `user`'s time, on `ticket` or as non-ticket time.
pub async fn log(
    conn: &mut PgConnection,
    user: Uuid,
    ticket: Option<&Ticket>,
    input: &TimeEntryInput,
) -> DeskResult<TimeEntry> {
    let c = check(conn, input, ticket.is_some()).await?;
    unlocked(conn, user, &c.worked_on).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_time_entries (id, user_id, org_id, ticket_id, work_type_id, worked_on, minutes, billable, notes) \
         VALUES ($1, $2, $3, $4, $5, $6::date, $7, $8, $9)",
    )
    .bind(id)
    .bind(user)
    .bind(ticket.map(|t| t.org_id))
    .bind(ticket.map(|t| t.id))
    .bind(input.work_type_id)
    .bind(&c.worked_on)
    .bind(input.minutes)
    .bind(c.billable)
    .bind(&c.notes)
    .execute(&mut *conn)
    .await?;
    entry(conn, id).await
}

/// Changes one of `user`'s entries (send the `version` you read).
pub async fn update(
    conn: &mut PgConnection,
    user: Uuid,
    id: Uuid,
    input: &TimeEntryInput,
) -> DeskResult<TimeEntry> {
    let before = entry(conn, id).await?;
    if before.user_id != user {
        return Err(DeskError::NotFound);
    }
    let c = check(conn, input, before.ticket_id.is_some()).await?;
    unlocked(conn, user, &before.worked_on).await?;
    unlocked(conn, user, &c.worked_on).await?;
    let rows = sqlx::query(
        "UPDATE sd_time_entries SET work_type_id = $3, worked_on = $4::date, minutes = $5, billable = $6, \
                notes = $7, version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(input.work_type_id)
    .bind(&c.worked_on)
    .bind(input.minutes)
    .bind(c.billable)
    .bind(&c.notes)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    entry(conn, id).await
}

/// Deletes one of `user`'s entries.
pub async fn delete(conn: &mut PgConnection, user: Uuid, id: Uuid) -> DeskResult<()> {
    let before = entry(conn, id).await?;
    if before.user_id != user {
        return Err(DeskError::NotFound);
    }
    unlocked(conn, user, &before.worked_on).await?;
    let rows = sqlx::query("DELETE FROM sd_time_entries WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .execute(conn)
        .await?
        .rows_affected();
    if rows == 0 {
        Err(DeskError::NotFound)
    } else {
        Ok(())
    }
}
