//! Dispatch (design doc §8.2): who a queue's new tickets go to. A queue's
//! strategy is `manual` (nobody is chosen), `round_robin` (whoever has
//! waited longest), `load` (whoever has the fewest open tickets) or
//! `skills` (whoever fits best; see the `workforce` module). This module
//! knows the queue's side: its members, their load and turns. Run in
//! system scope: queues usually belong to the MSP, tickets to clients.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult};

pub const STRATEGIES: &[&str] = &["manual", "round_robin", "load", "skills"];

/// Someone a queue's ticket could go to.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub user_id: Uuid,
    pub name: String,
    /// Tickets assigned to them that aren't resolved or closed.
    pub open_tickets: i64,
    /// When dispatch last gave them one of this queue's tickets.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_assigned_at: Option<time::OffsetDateTime>,
}

/// A queue's dispatch strategy.
pub async fn strategy(conn: &mut PgConnection, queue: Uuid) -> DeskResult<String> {
    sqlx::query_scalar("SELECT dispatch FROM sd_queues WHERE id = $1 AND archived_at IS NULL")
        .bind(queue)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// A queue's active members (people, and the people in its teams), in
/// round-robin order: who has waited longest first, then by name.
pub async fn candidates(conn: &mut PgConnection, queue: Uuid) -> DeskResult<Vec<Candidate>> {
    Ok(sqlx::query_as(
        "WITH members AS ( \
           SELECT m.user_id FROM sd_queue_members m WHERE m.queue_id = $1 AND m.user_id IS NOT NULL \
           UNION \
           SELECT tm.user_id FROM sd_queue_members m JOIN team_members tm ON tm.team_id = m.team_id \
            WHERE m.queue_id = $1) \
         SELECT u.id AS user_id, u.display_name AS name, \
                (SELECT count(*) FROM sd_tickets t WHERE t.assignee_user_id = u.id \
                   AND t.resolved_at IS NULL AND t.closed_at IS NULL) AS open_tickets, \
                d.last_assigned_at \
           FROM members JOIN users u ON u.id = members.user_id \
           LEFT JOIN sd_dispatch_turns d ON d.queue_id = $1 AND d.user_id = u.id \
          WHERE u.status = 'active' AND u.kind = 'human' \
          ORDER BY d.last_assigned_at NULLS FIRST, lower(u.display_name), u.id",
    )
    .bind(queue)
    .fetch_all(conn)
    .await?)
}

/// Orders candidates for `load`: fewest open tickets, then round-robin.
pub fn by_load(candidates: &mut [Candidate]) {
    // Stable: ties keep round-robin order.
    candidates.sort_by_key(|c| c.open_tickets);
}

/// Records that dispatch gave `user` one of `queue`'s tickets.
pub async fn took_turn(conn: &mut PgConnection, queue: Uuid, user: Uuid) -> DeskResult<()> {
    sqlx::query(
        "INSERT INTO sd_dispatch_turns (queue_id, org_id, user_id, last_assigned_at) \
         SELECT id, org_id, $2, clock_timestamp() FROM sd_queues WHERE id = $1 \
         ON CONFLICT (queue_id, user_id) DO UPDATE SET last_assigned_at = EXCLUDED.last_assigned_at",
    )
    .bind(queue)
    .bind(user)
    .execute(conn)
    .await?;
    Ok(())
}
