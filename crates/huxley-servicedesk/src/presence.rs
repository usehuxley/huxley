//! Who has a ticket open, and who's typing on it: the collision indicator
//! (design doc §8.2). Pages check in every few seconds; anyone not heard
//! from in [`FRESH_SECONDS`] has gone. System scope (the caller has checked
//! access to the ticket).

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::DeskResult;

/// How long a check-in counts.
pub const FRESH_SECONDS: i32 = 40;

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow, PartialEq, Eq)]
#[schema(as = TicketViewer)]
#[serde(rename_all = "camelCase")]
pub struct Viewer {
    pub user_id: Uuid,
    pub name: String,
    /// Writing a note or reply right now.
    pub typing: bool,
}

/// `user` is on `ticket` (typing or not), or has left it. Returns the
/// others there now.
pub async fn check_in(
    conn: &mut PgConnection,
    ticket: Uuid,
    user: Uuid,
    typing: bool,
    leaving: bool,
) -> DeskResult<Vec<Viewer>> {
    if leaving {
        sqlx::query("DELETE FROM sd_ticket_presence WHERE ticket_id = $1 AND user_id = $2")
            .bind(ticket)
            .bind(user)
            .execute(&mut *conn)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO sd_ticket_presence (ticket_id, user_id, typing, seen_at) VALUES ($1, $2, $3, now()) \
             ON CONFLICT (ticket_id, user_id) DO UPDATE SET typing = EXCLUDED.typing, seen_at = now()",
        )
        .bind(ticket)
        .bind(user)
        .bind(typing)
        .execute(&mut *conn)
        .await?;
    }
    // Tidy up as we go: nobody reads rows this old.
    sqlx::query("DELETE FROM sd_ticket_presence WHERE seen_at < now() - interval '1 hour'")
        .execute(&mut *conn)
        .await?;
    Ok(sqlx::query_as(
        "SELECT p.user_id, u.display_name AS name, p.typing FROM sd_ticket_presence p \
           JOIN users u ON u.id = p.user_id \
          WHERE p.ticket_id = $1 AND p.user_id <> $2 AND p.seen_at > now() - make_interval(secs => $3) \
          ORDER BY p.typing DESC, lower(u.display_name)",
    )
    .bind(ticket)
    .bind(user)
    .bind(f64::from(FRESH_SECONDS))
    .fetch_all(conn)
    .await?)
}
