//! Appointments (design doc §8.2, the dispatch calendar): scheduled work on
//! a ticket, for a person, such as an onsite visit. Nobody is booked twice
//! at once. Each change is noted on the ticket.

use huxley_core::RequestContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{self, Ticket, record};
use crate::{DeskError, DeskResult, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Appointment {
    pub id: Uuid,
    pub ticket_id: Uuid,
    pub ticket_reference: String,
    pub ticket_title: String,
    pub org_name: String,
    pub user_id: Uuid,
    pub user_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
    pub notes: String,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppointmentInput {
    /// Who does the work.
    pub user_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    /// After `startsAt`, within 24 hours of it.
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
    #[serde(default)]
    pub notes: Option<String>,
    /// The version you read (rescheduling only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT a.id, a.ticket_id, t.number_prefix || '-' || t.number AS ticket_reference, \
    t.title AS ticket_title, o.name AS org_name, a.user_id, u.display_name AS user_name, a.starts_at, \
    a.ends_at, a.notes, a.version \
    FROM sd_appointments a JOIN sd_tickets t ON t.id = a.ticket_id JOIN organizations o ON o.id = a.org_id \
    JOIN users u ON u.id = a.user_id";

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<Appointment> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// A ticket's appointments, in order.
pub async fn for_ticket(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Vec<Appointment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.ticket_id = $1 ORDER BY a.starts_at"
    )))
    .bind(ticket)
    .fetch_all(conn)
    .await?)
}

/// Appointments overlapping `from`–`to`, of `people` (or everyone), in
/// order. What the transaction's scope can see.
pub async fn between(
    conn: &mut PgConnection,
    from: OffsetDateTime,
    to: OffsetDateTime,
    people: Option<&[Uuid]>,
) -> DeskResult<Vec<Appointment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.starts_at < $2 AND a.ends_at > $1 AND ($3::uuid[] IS NULL OR a.user_id = ANY($3)) \
          ORDER BY a.starts_at, u.display_name"
    )))
    .bind(from)
    .bind(to)
    .bind(people.map(<[Uuid]>::to_vec))
    .fetch_all(conn)
    .await?)
}

struct Checked {
    notes: String,
    person: String,
}

/// Checks a booking: the times, the person, and that they're free. System
/// scope (a clash may be on another client's ticket).
async fn check(
    conn: &mut PgConnection,
    input: &AppointmentInput,
    except: Option<Uuid>,
) -> DeskResult<Checked> {
    if input.ends_at <= input.starts_at {
        return Err(DeskError::invalid("endsAt", "after the start"));
    }
    if input.ends_at - input.starts_at > time::Duration::hours(24) {
        return Err(DeskError::invalid("endsAt", "within 24 hours of the start"));
    }
    let person: Option<String> = sqlx::query_scalar(
        "SELECT display_name FROM users WHERE id = $1 AND status = 'active' AND kind = 'human'",
    )
    .bind(input.user_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(person) = person else {
        return Err(DeskError::invalid("userId", "there's no such person"));
    };
    let clash: Option<String> = sqlx::query_scalar(
        "SELECT t.number_prefix || '-' || t.number FROM sd_appointments a JOIN sd_tickets t ON t.id = a.ticket_id \
          WHERE a.user_id = $1 AND a.starts_at < $3 AND a.ends_at > $2 AND ($4::uuid IS NULL OR a.id <> $4) \
          ORDER BY a.starts_at LIMIT 1",
    )
    .bind(input.user_id)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(except)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(reference) = clash {
        return Err(DeskError::invalid(
            "startsAt",
            format!("{person} is booked on {reference} then"),
        ));
    }
    Ok(Checked {
        notes: text("notes", input.notes.as_deref(), 2000)?.unwrap_or_default(),
        person,
    })
}

fn when(at: OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Books someone on a ticket.
pub async fn book(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &Ticket,
    input: &AppointmentInput,
) -> DeskResult<Appointment> {
    let c = check(conn, input, None).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_appointments (id, ticket_id, org_id, user_id, starts_at, ends_at, notes, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(ticket.id)
    .bind(ticket.org_id)
    .bind(input.user_id)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(&c.notes)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    record(
        conn,
        ctx,
        ticket,
        "appointment",
        json!({ "booked": c.person, "from": when(input.starts_at), "to": when(input.ends_at) }),
    )
    .await?;
    get(conn, id).await
}

/// Moves an appointment (send the `version` you read).
pub async fn reschedule(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &AppointmentInput,
) -> DeskResult<Appointment> {
    let before = get(conn, id).await?;
    let c = check(conn, input, Some(id)).await?;
    let rows = sqlx::query(
        "UPDATE sd_appointments SET user_id = $3, starts_at = $4, ends_at = $5, notes = $6, version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(input.user_id)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(&c.notes)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    let ticket = tickets::get(conn, before.ticket_id).await?;
    record(
        conn,
        ctx,
        &ticket,
        "appointment",
        json!({ "moved": c.person, "from": when(input.starts_at), "to": when(input.ends_at) }),
    )
    .await?;
    get(conn, id).await
}

/// Cancels an appointment.
pub async fn cancel(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let found = get(conn, id).await?;
    sqlx::query("DELETE FROM sd_appointments WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let ticket = tickets::get(conn, found.ticket_id).await?;
    record(
        conn,
        ctx,
        &ticket,
        "appointment",
        json!({ "cancelled": found.user_name, "from": when(found.starts_at) }),
    )
    .await?;
    Ok(())
}
