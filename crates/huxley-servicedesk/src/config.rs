//! Instance-wide configuration: ticket types with their statuses, allowed
//! status changes, and the priority matrix.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    /// `new`, `open`, `pending`, `resolved` or `closed`.
    pub category: String,
    /// The SLA clock stops while a ticket is here.
    pub pauses_sla: bool,
    /// Needed to enter it: `resolution` (a resolution note), `assignee`.
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TicketType {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    #[sqlx(skip)]
    pub statuses: Vec<Status>,
    /// Allowed status changes (from key → to keys). Empty: any change.
    #[sqlx(skip)]
    #[schema(value_type = std::collections::HashMap<String, Vec<String>>)]
    pub transitions: std::collections::BTreeMap<String, Vec<String>>,
}

/// Every active ticket type with its statuses, in order.
pub async fn types(conn: &mut PgConnection) -> DeskResult<Vec<TicketType>> {
    let mut types: Vec<TicketType> = sqlx::query_as(
        "SELECT id, key, name, description FROM sd_ticket_types \
          WHERE archived_at IS NULL ORDER BY position, name",
    )
    .fetch_all(&mut *conn)
    .await?;
    for t in &mut types {
        t.statuses = statuses(conn, t.id).await?;
        let pairs: Vec<(String, String)> = sqlx::query_as(
            "SELECT f.key, s.key FROM sd_transitions x \
               JOIN sd_statuses f ON f.id = x.from_status_id \
               JOIN sd_statuses s ON s.id = x.to_status_id \
              WHERE x.type_id = $1 ORDER BY f.position, s.position",
        )
        .bind(t.id)
        .fetch_all(&mut *conn)
        .await?;
        for (from, to) in pairs {
            t.transitions.entry(from).or_default().push(to);
        }
    }
    Ok(types)
}

/// A type's statuses, in order.
pub async fn statuses(conn: &mut PgConnection, type_id: Uuid) -> DeskResult<Vec<Status>> {
    Ok(sqlx::query_as(
        "SELECT id, key, name, category, pauses_sla, requires FROM sd_statuses \
          WHERE type_id = $1 ORDER BY position",
    )
    .bind(type_id)
    .fetch_all(conn)
    .await?)
}

/// The type with this key.
pub async fn type_by_key(conn: &mut PgConnection, key: &str) -> DeskResult<(Uuid, String)> {
    sqlx::query_as("SELECT id, name FROM sd_ticket_types WHERE key = $1 AND archived_at IS NULL")
        .bind(key)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DeskError::invalid("type", format!("there's no ticket type {key:?}")))
}

/// Whether a ticket of `type_id` may go from `from` to `to`.
pub async fn may_change(
    conn: &mut PgConnection,
    type_id: Uuid,
    from: Uuid,
    to: Uuid,
) -> DeskResult<bool> {
    let (restricted, allowed): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM sd_transitions WHERE type_id = $1), \
                EXISTS (SELECT 1 FROM sd_transitions WHERE from_status_id = $2 AND to_status_id = $3)",
    )
    .bind(type_id)
    .bind(from)
    .bind(to)
    .fetch_one(conn)
    .await?;
    Ok(!restricted || allowed || from == to)
}

/// The priority for an impact and urgency (1 high … 3 low).
pub async fn priority_for(
    conn: &mut PgConnection,
    impact: i16,
    urgency: i16,
) -> DeskResult<String> {
    sqlx::query_scalar("SELECT priority FROM sd_priority_matrix WHERE impact = $1 AND urgency = $2")
        .bind(impact)
        .bind(urgency)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DeskError::invalid("impact", "impact and urgency are 1 (high) to 3 (low)"))
}

/// One cell of the priority matrix.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCell {
    pub impact: i16,
    pub urgency: i16,
    pub priority: String,
}

pub async fn matrix(conn: &mut PgConnection) -> DeskResult<Vec<MatrixCell>> {
    Ok(sqlx::query_as(
        "SELECT impact, urgency, priority FROM sd_priority_matrix ORDER BY impact, urgency",
    )
    .fetch_all(conn)
    .await?)
}
