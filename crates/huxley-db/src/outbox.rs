//! Domain events outbox (design doc §4.3).
//!
//! Events are written in the same transaction as the change that causes them,
//! so an event is never published for a change that rolled back, and never
//! lost for one that committed. The dispatcher (Phase 2) publishes them to
//! workflow event triggers.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::DbError;

/// A domain event, e.g. `org.created` about an `organization`.
#[derive(Debug, Clone)]
pub struct DomainEvent<'a> {
    pub event_type: &'a str,
    pub org_id: Option<Uuid>,
    pub subject_type: &'a str,
    pub subject_id: Uuid,
    pub payload: serde_json::Value,
}

/// Writes an event to the outbox. Returns its id.
pub async fn emit(conn: &mut PgConnection, event: DomainEvent<'_>) -> Result<Uuid, DbError> {
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO events_outbox (id, org_id, event_type, subject_type, subject_id, payload) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(event.org_id)
    .bind(event.event_type)
    .bind(event.subject_type)
    .bind(event.subject_id)
    .bind(&event.payload)
    .execute(conn)
    .await?;
    Ok(id)
}
