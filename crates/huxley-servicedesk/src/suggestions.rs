//! Suggested time entries (ADR-051): drafted for whoever resolved a ticket
//! without logging time on it, kept apart from real time until they accept
//! one (then it's a time entry like any other) or dismiss it.

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::time::{self, TimeEntry, TimeEntryInput};
use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TimeSuggestion)]
pub struct Suggestion {
    pub id: Uuid,
    pub ticket_id: Uuid,
    pub ticket_reference: String,
    pub ticket_title: String,
    pub user_id: Uuid,
    pub worked_on: String,
    pub minutes: i32,
    pub notes: String,
}

const SELECT: &str = "SELECT s.id, s.ticket_id, t.number_prefix || '-' || t.number AS ticket_reference, \
     t.title AS ticket_title, s.user_id, to_char(s.worked_on, 'YYYY-MM-DD') AS worked_on, s.minutes, s.notes \
     FROM sd_time_suggestions s JOIN sd_tickets t ON t.id = s.ticket_id";

/// Suggests time on `ticket` for `user` (by default its assignee), unless
/// they've logged time on it already or a suggestion is waiting. `Ok(None)`:
/// nothing suggested.
pub async fn suggest(
    conn: &mut PgConnection,
    ticket: Uuid,
    user: Option<Uuid>,
    minutes: i32,
    notes: &str,
) -> DeskResult<Option<Suggestion>> {
    if !(1..=1440).contains(&minutes) {
        return Err(DeskError::invalid("minutes", "1 to 1440"));
    }
    let row: Option<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT org_id, assignee_user_id FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_optional(&mut *conn)
            .await?;
    let (org, assignee) = row.ok_or(DeskError::NotFound)?;
    let Some(user) = user.or(assignee) else {
        return Ok(None);
    };
    let logged: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sd_time_entries WHERE ticket_id = $1 AND user_id = $2)",
    )
    .bind(ticket)
    .bind(user)
    .fetch_one(&mut *conn)
    .await?;
    if logged {
        return Ok(None);
    }
    let notes: String = notes.trim().chars().take(4000).collect();
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO sd_time_suggestions (id, org_id, ticket_id, user_id, worked_on, minutes, notes) \
         VALUES ($1, $2, $3, $4, current_date, $5, $6) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(ticket)
    .bind(user)
    .bind(minutes)
    .bind(&notes)
    .fetch_optional(&mut *conn)
    .await?;
    match id {
        Some(id) => one(conn, id).await.map(Some),
        None => Ok(None),
    }
}

async fn one(conn: &mut PgConnection, id: Uuid) -> DeskResult<Suggestion> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE s.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// `user`'s waiting suggestions, newest first.
pub async fn mine(conn: &mut PgConnection, user: Uuid) -> DeskResult<Vec<Suggestion>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.user_id = $1 AND s.status = 'pending' ORDER BY s.created_at DESC"
    )))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AcceptTimeSuggestion)]
pub struct Accept {
    pub work_type_id: Uuid,
    /// Change the minutes before accepting.
    #[serde(default)]
    pub minutes: Option<i32>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Accepts `user`'s suggestion as a time entry.
pub async fn accept(
    conn: &mut PgConnection,
    user: Uuid,
    id: Uuid,
    input: &Accept,
) -> DeskResult<TimeEntry> {
    let s = one(&mut *conn, id).await?;
    let pending: bool = sqlx::query_scalar(
        "SELECT status = 'pending' FROM sd_time_suggestions WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(false);
    if !pending {
        return Err(DeskError::NotFound);
    }
    let ticket = crate::tickets::get(conn, s.ticket_id).await?;
    let entry = time::log(
        conn,
        user,
        Some(&ticket),
        &TimeEntryInput {
            work_type_id: input.work_type_id,
            worked_on: Some(s.worked_on.clone()),
            minutes: input.minutes.unwrap_or(s.minutes),
            billable: None,
            notes: Some(input.notes.clone().unwrap_or(s.notes)),
            version: None,
        },
    )
    .await?;
    sqlx::query("UPDATE sd_time_suggestions SET status = 'accepted', entry_id = $2 WHERE id = $1")
        .bind(id)
        .bind(entry.id)
        .execute(conn)
        .await?;
    Ok(entry)
}

/// Dismisses `user`'s suggestion.
pub async fn dismiss(conn: &mut PgConnection, user: Uuid, id: Uuid) -> DeskResult<bool> {
    Ok(sqlx::query(
        "UPDATE sd_time_suggestions SET status = 'dismissed' WHERE id = $1 AND user_id = $2 AND status = 'pending'",
    )
    .bind(id)
    .bind(user)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}
