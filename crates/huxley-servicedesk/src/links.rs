//! Linked and merged tickets (design doc §8.2).
//!
//! - A link says another ticket is related, or this one's parent or child.
//!   It's kept as one row per side, so each ticket's organization governs
//!   who sees it. A ticket has at most one parent, and parents can't loop.
//! - Merging folds a duplicate into another ticket of the same
//!   organization: its notes, attachments and emails move over, and it's
//!   closed, pointing at the one it was merged into. Replies to it thread
//!   into that one.

use huxley_core::RequestContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{self, Ticket, record};
use crate::{DeskError, DeskResult};

pub const KINDS: &[&str] = &["related", "parent", "child", "problem", "incident"];

/// Another ticket linked to this one.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct LinkedTicket {
    /// What it is to this ticket: `related`, `parent` or `child`.
    pub kind: String,
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub org_name: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewLink {
    /// The other ticket, by id…
    #[serde(default)]
    pub other_id: Option<Uuid>,
    /// …or by reference (e.g. `T-1042`, or just its number).
    #[serde(default)]
    pub reference: Option<String>,
    /// What the other ticket is to this one: `related` (the default),
    /// `parent` or `child`.
    #[serde(default)]
    pub kind: Option<String>,
}

fn inverse(kind: &str) -> &'static str {
    match kind {
        "parent" => "child",
        "child" => "parent",
        // An incident's problem (its cause); a problem's incidents.
        "problem" => "incident",
        "incident" => "problem",
        _ => "related",
    }
}

/// The ticket a reference (`T-1042`, `1042`) names, if any is visible.
/// `field` is the input it came from, for errors.
pub async fn by_reference(
    conn: &mut PgConnection,
    field: &str,
    reference: &str,
) -> DeskResult<Uuid> {
    let number: i64 = reference
        .trim()
        .rsplit('-')
        .next()
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| DeskError::invalid(field, "a ticket reference, like T-1042"))?;
    sqlx::query_scalar("SELECT id FROM sd_tickets WHERE number = $1")
        .bind(number)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| DeskError::invalid(field, "there's no such ticket"))
}

/// Tickets linked to `id`: parent first, then children, then related.
pub async fn list(conn: &mut PgConnection, id: Uuid) -> DeskResult<Vec<LinkedTicket>> {
    Ok(sqlx::query_as(
        "SELECT l.kind, t.id, t.number_prefix || '-' || t.number AS reference, t.title, \
                s.name AS status_name, s.category AS status_category, o.name AS org_name \
           FROM sd_ticket_links l \
           JOIN sd_tickets t ON t.id = l.other_id \
           JOIN sd_statuses s ON s.id = t.status_id \
           JOIN organizations o ON o.id = t.org_id \
          WHERE l.ticket_id = $1 \
          ORDER BY array_position(ARRAY['problem', 'parent', 'child', 'incident', 'related'], l.kind), t.number",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Whether `ancestor` is `id` or above it through parents.
async fn is_ancestor_or_self(
    conn: &mut PgConnection,
    ancestor: Uuid,
    id: Uuid,
) -> DeskResult<bool> {
    Ok(sqlx::query_scalar(
        "WITH RECURSIVE up(id, depth) AS ( \
           SELECT $2::uuid, 0 \
           UNION ALL \
           SELECT l.other_id, up.depth + 1 FROM sd_ticket_links l JOIN up ON l.ticket_id = up.id \
            WHERE l.kind = 'parent' AND up.depth < 50) \
         SELECT EXISTS (SELECT 1 FROM up WHERE id = $1)",
    )
    .bind(ancestor)
    .bind(id)
    .fetch_one(conn)
    .await?)
}

/// Links `other` to `ticket` as its `kind`. Both must be visible.
pub async fn link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    other: Uuid,
    kind: &str,
) -> DeskResult<Vec<LinkedTicket>> {
    if !KINDS.contains(&kind) {
        return Err(DeskError::invalid(
            "kind",
            "related, parent, child, problem or incident",
        ));
    }
    if ticket == other {
        return Err(DeskError::invalid(
            "reference",
            "a ticket can't be linked to itself",
        ));
    }
    let (a, b) = (
        tickets::get(conn, ticket).await?,
        tickets::get(conn, other).await?,
    );
    // Parents can't loop: the new parent mustn't be below the child.
    let (parent, child) = match kind {
        "parent" => (Some(other), ticket),
        "child" => (Some(ticket), other),
        _ => (None, ticket),
    };
    if let Some(parent) = parent {
        if is_ancestor_or_self(conn, child, parent).await? {
            return Err(DeskError::invalid(
                "kind",
                "that would make a ticket its own parent",
            ));
        }
        let has_parent: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sd_ticket_links WHERE ticket_id = $1 AND kind = 'parent' AND other_id <> $2)",
        )
        .bind(child)
        .bind(parent)
        .fetch_one(&mut *conn)
        .await?;
        if has_parent {
            return Err(DeskError::invalid(
                "kind",
                "that ticket already has a parent",
            ));
        }
    }
    for (this, that, k) in [(&a, &b, kind), (&b, &a, inverse(kind))] {
        sqlx::query(
            "INSERT INTO sd_ticket_links (ticket_id, other_id, org_id, kind) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (ticket_id, other_id) DO UPDATE SET kind = EXCLUDED.kind",
        )
        .bind(this.id)
        .bind(that.id)
        .bind(this.org_id)
        .bind(k)
        .execute(&mut *conn)
        .await?;
        record(
            conn,
            ctx,
            this,
            "link",
            json!({ "linked": that.reference, "as": k }),
        )
        .await?;
    }
    list(conn, ticket).await
}

/// Removes the link between two tickets (both sides).
pub async fn unlink(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    other: Uuid,
) -> DeskResult<()> {
    let (a, b) = (
        tickets::get(conn, ticket).await?,
        tickets::get(conn, other).await?,
    );
    let removed = sqlx::query(
        "DELETE FROM sd_ticket_links WHERE (ticket_id = $1 AND other_id = $2) OR (ticket_id = $2 AND other_id = $1)",
    )
    .bind(ticket)
    .bind(other)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(DeskError::NotFound);
    }
    for (this, that) in [(&a, &b), (&b, &a)] {
        record(
            conn,
            ctx,
            this,
            "link",
            json!({ "unlinked": that.reference }),
        )
        .await?;
    }
    Ok(())
}

/// Merges `source` into `target`: notes, attachments and emails move over,
/// and `source` is closed pointing at `target`. Both must be visible, open
/// to change, and of the same organization. Returns `target`.
pub async fn merge(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    source: Uuid,
    target: Uuid,
) -> DeskResult<Ticket> {
    if source == target {
        return Err(DeskError::invalid(
            "into",
            "a ticket can't be merged into itself",
        ));
    }
    let (from, into) = (
        tickets::get(conn, source).await?,
        tickets::get(conn, target).await?,
    );
    if from.org_id != into.org_id {
        return Err(DeskError::invalid(
            "into",
            "only tickets of the same organization can be merged",
        ));
    }
    if from.merged_into_id.is_some() {
        return Err(DeskError::invalid(
            "into",
            format!("{} was already merged", from.reference),
        ));
    }
    if into.merged_into_id.is_some() || into.status_category == "closed" {
        return Err(DeskError::invalid(
            "into",
            format!("{} is closed", into.reference),
        ));
    }
    for table in ["sd_ticket_notes", "sd_inbound_emails"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET ticket_id = $2 WHERE ticket_id = $1"
        )))
        .bind(source)
        .bind(target)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE attachments SET entity_id = $2 WHERE entity_type = 'ticket' AND entity_id = $1",
    )
    .bind(source)
    .bind(target)
    .execute(&mut *conn)
    .await?;
    // Closed, whatever the type's usual path: merging isn't a status change
    // someone chose.
    let closed: Uuid = sqlx::query_scalar(
        "SELECT s.id FROM sd_statuses s JOIN sd_tickets t ON t.type_id = s.type_id \
          WHERE t.id = $1 AND s.category = 'closed' ORDER BY s.position LIMIT 1",
    )
    .bind(source)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE sd_tickets SET status_id = $2, merged_into_id = $3, version = version + 1, updated_at = now(), \
                resolved_at = coalesce(resolved_at, now()), closed_at = coalesce(closed_at, now()) \
          WHERE id = $1",
    )
    .bind(source)
    .bind(closed)
    .bind(target)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sd_tickets SET version = version + 1, updated_at = now() WHERE id = $1")
        .bind(target)
        .execute(&mut *conn)
        .await?;
    crate::sla::sync(conn, source).await?;
    let was = from.status_key;
    let (from, into) = (
        tickets::get(conn, source).await?,
        tickets::get(conn, target).await?,
    );
    record(
        conn,
        ctx,
        &from,
        "status",
        json!({ "from": was, "to": from.status_key, "category": "closed", "mergedInto": into.reference }),
    )
    .await?;
    record(
        conn,
        ctx,
        &into,
        "merge",
        json!({ "merged": from.reference }),
    )
    .await?;
    Ok(into)
}
