//! My Work (design doc §8.2): a technician's tickets, most in need of
//! attention first; their teams' tickets nobody has picked up; and what's
//! waiting, unassigned, in the queues they're a member of.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::DeskResult;
use crate::tickets::{SELECT, Ticket};

/// How soon an SLA target counts as due soon.
pub const DUE_SOON: Duration = Duration::hours(4);
/// Tickets shown per queue.
const PER_QUEUE: i64 = 5;

const OPEN: &str = "s.category IN ('new', 'open', 'pending')";

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyWork {
    /// Open tickets assigned to you, most in need of attention first.
    pub mine: Vec<Ticket>,
    /// Why a ticket of yours needs attention, by id: `breached` (an SLA
    /// target was missed), `replied` (the requester wrote last), `due_soon`
    /// (an SLA target is due within 4 hours) or `new` (not yet started).
    pub attention: BTreeMap<Uuid, String>,
    /// Open tickets assigned to one of your teams but to nobody in it.
    pub team: Vec<Ticket>,
    /// The queues you're a member of (or through a team), with their
    /// unassigned open tickets.
    pub queues: Vec<QueueWork>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueueWork {
    pub queue_id: Uuid,
    pub name: String,
    /// How many open tickets in it nobody (person or team) has.
    pub unassigned: i64,
    /// The most urgent of them, oldest first within a priority.
    pub tickets: Vec<Ticket>,
}

/// When the ticket's next SLA target is due: the first response until
/// there is one, then resolution.
fn due(t: &Ticket) -> Option<OffsetDateTime> {
    if t.first_response_at.is_none() && t.response_due_at.is_some() {
        t.response_due_at
    } else {
        t.resolution_due_at
    }
}

fn breached(t: &Ticket) -> bool {
    t.resolution_breached_at.is_some()
        || (t.response_breached_at.is_some() && t.first_response_at.is_none())
}

/// Why a ticket needs attention at `now`, most pressing reason first.
fn attention(t: &Ticket, replied: bool, now: OffsetDateTime) -> Option<&'static str> {
    if breached(t) {
        Some("breached")
    } else if replied {
        Some("replied")
    } else if due(t).is_some_and(|d| d <= now + DUE_SOON) {
        Some("due_soon")
    } else if t.status_category == "new" {
        Some("new")
    } else {
        None
    }
}

fn rank(reason: Option<&str>) -> u8 {
    match reason {
        Some("breached") => 0,
        Some("replied") => 1,
        Some("due_soon") => 2,
        Some("new") => 3,
        _ => 4,
    }
}

/// `user`'s work, as of `now`, from what the transaction's scope can see.
pub async fn of(conn: &mut PgConnection, user: Uuid, now: OffsetDateTime) -> DeskResult<MyWork> {
    let mut mine: Vec<Ticket> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.assignee_user_id = $1 AND {OPEN} LIMIT 500"
    )))
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = mine.iter().map(|t| t.id).collect();
    // The requester wrote last: the latest public note is theirs.
    let latest_from_requester: BTreeSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT ticket_id FROM ( \
           SELECT DISTINCT ON (n.ticket_id) n.ticket_id, n.author_contact_id FROM sd_ticket_notes n \
            WHERE n.ticket_id = ANY($1) AND n.kind <> 'internal' \
            ORDER BY n.ticket_id, n.created_at DESC, n.id DESC) last \
          WHERE last.author_contact_id IS NOT NULL",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let mut attention_of = BTreeMap::new();
    for t in &mine {
        if let Some(why) = attention(t, latest_from_requester.contains(&t.id), now) {
            attention_of.insert(t.id, why.to_owned());
        }
    }
    mine.sort_by(|a, b| {
        rank(attention_of.get(&a.id).map(String::as_str))
            .cmp(&rank(attention_of.get(&b.id).map(String::as_str)))
            .then_with(|| match (due(a), due(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.priority.cmp(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    let team: Vec<Ticket> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.assignee_user_id IS NULL AND {OPEN} \
           AND t.assignee_team_id IN (SELECT team_id FROM team_members WHERE user_id = $1) \
         ORDER BY t.priority, t.created_at LIMIT 200"
    )))
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;

    let queues: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT q.id, q.name, \
                (SELECT count(*) FROM sd_tickets t JOIN sd_statuses s ON s.id = t.status_id \
                  WHERE t.queue_id = q.id AND t.assignee_user_id IS NULL AND t.assignee_team_id IS NULL \
                    AND s.category IN ('new', 'open', 'pending')) \
           FROM sd_queues q \
          WHERE q.archived_at IS NULL AND EXISTS ( \
                SELECT 1 FROM sd_queue_members m \
                 WHERE m.queue_id = q.id AND (m.user_id = $1 OR m.team_id IN \
                       (SELECT team_id FROM team_members WHERE user_id = $1))) \
          ORDER BY lower(q.name)",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    let mut work = Vec::with_capacity(queues.len());
    for (queue_id, name, unassigned) in queues {
        let tickets = if unassigned == 0 {
            Vec::new()
        } else {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "{SELECT} WHERE t.queue_id = $1 AND t.assignee_user_id IS NULL \
                   AND t.assignee_team_id IS NULL AND {OPEN} \
                 ORDER BY t.priority, t.created_at LIMIT {PER_QUEUE}"
            )))
            .bind(queue_id)
            .fetch_all(&mut *conn)
            .await?
        };
        work.push(QueueWork {
            queue_id,
            name,
            unassigned,
            tickets,
        });
    }
    Ok(MyWork {
        mine,
        attention: attention_of,
        team,
        queues: work,
    })
}
