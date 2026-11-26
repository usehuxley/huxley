//! Tickets, their notes and their activity (ADR-036).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::config;
use crate::{DeskError, DeskResult, text};

pub const PRIORITIES: &[&str] = &["p1", "p2", "p3", "p4"];
pub const SOURCES: &[&str] = &["web", "email", "portal", "api", "workflow", "phone"];
pub const NOTE_KINDS: &[&str] = &["public", "internal", "resolution"];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: Uuid,
    /// The ticket's number with its prefix, e.g. `T-1042`.
    pub reference: String,
    pub number: i64,
    pub org_id: Uuid,
    pub org_name: String,
    pub type_key: String,
    pub type_name: String,
    pub status_key: String,
    pub status_name: String,
    /// `new`, `open`, `pending`, `resolved` or `closed`.
    pub status_category: String,
    pub title: String,
    pub description: String,
    /// 1 (high) to 3 (low).
    pub impact: i16,
    /// 1 (high) to 3 (low).
    pub urgency: i16,
    /// `p1` to `p4`.
    pub priority: String,
    /// Chosen by someone rather than following the matrix.
    pub priority_overridden: bool,
    pub queue_id: Option<Uuid>,
    pub queue_name: Option<String>,
    pub requester_id: Option<Uuid>,
    pub requester_name: Option<String>,
    pub requester_email: Option<String>,
    pub assignee_user_id: Option<Uuid>,
    pub assignee_user_name: Option<String>,
    pub assignee_team_id: Option<Uuid>,
    pub assignee_team_name: Option<String>,
    pub source: String,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub custom_fields: Json,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub first_response_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    /// The SLA policy the ticket was opened under, if any.
    pub sla_policy: Option<String>,
    /// When the first response is due.
    #[serde(with = "time::serde::rfc3339::option")]
    pub response_due_at: Option<OffsetDateTime>,
    /// Whether the first response met its target (once there is one).
    pub response_met: Option<bool>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub response_breached_at: Option<OffsetDateTime>,
    /// When resolution is due; none while the clock is paused or stopped.
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolution_due_at: Option<OffsetDateTime>,
    /// Whether resolution met its target (once resolved).
    pub resolution_met: Option<bool>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolution_breached_at: Option<OffsetDateTime>,
    /// The resolution clock is paused (by a waiting status).
    pub sla_paused: bool,
    /// The ticket this one was merged into (it's closed).
    pub merged_into_id: Option<Uuid>,
    pub merged_into_reference: Option<String>,
}

pub(crate) const SELECT: &str = "SELECT t.id, t.number_prefix || '-' || t.number AS reference, t.number, t.org_id, \
    o.name AS org_name, ty.key AS type_key, ty.name AS type_name, s.key AS status_key, \
    s.name AS status_name, s.category AS status_category, t.title, t.description, t.impact, t.urgency, \
    t.priority, t.priority_overridden, t.queue_id, q.name AS queue_name, t.requester_id, \
    nullif(btrim(c.first_name || ' ' || c.last_name), '') AS requester_name, c.email AS requester_email, \
    t.assignee_user_id, u.display_name AS assignee_user_name, t.assignee_team_id, tm.name AS assignee_team_name, \
    t.source, t.custom_fields, t.version, t.created_at, t.updated_at, t.first_response_at, t.resolved_at, \
    t.closed_at, sla.policy_name AS sla_policy, sla.response_due_at, sla.response_met, \
    sla.response_breached_at, sla.resolution_due_at, sla.resolution_met, sla.resolution_breached_at, \
    coalesce(sla.resolution_clock_at IS NULL AND sla.resolution_met IS NULL \
             AND sla.targets -> t.priority ->> 'resolution' IS NOT NULL, false) AS sla_paused, \
    t.merged_into_id, mi.number_prefix || '-' || mi.number AS merged_into_reference \
    FROM sd_tickets t \
    JOIN organizations o ON o.id = t.org_id \
    JOIN sd_ticket_types ty ON ty.id = t.type_id \
    JOIN sd_statuses s ON s.id = t.status_id \
    LEFT JOIN sd_queues q ON q.id = t.queue_id \
    LEFT JOIN crm_contacts c ON c.id = t.requester_id \
    LEFT JOIN users u ON u.id = t.assignee_user_id \
    LEFT JOIN teams tm ON tm.id = t.assignee_team_id \
    LEFT JOIN sd_ticket_sla sla ON sla.ticket_id = t.id \
    LEFT JOIN sd_tickets mi ON mi.id = t.merged_into_id";

/// Present-or-null fields of an update: missing leaves the field alone,
/// `null` clears it.
#[allow(clippy::option_option)] // JSON's three states: missing, null, a value.
fn some<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewTicket {
    /// The organization it's for.
    pub org_id: Uuid,
    /// A ticket type's key (default `incident`).
    #[serde(default)]
    pub r#type: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 1 (high) to 3 (low); default 2.
    #[serde(default)]
    pub impact: Option<i16>,
    /// 1 (high) to 3 (low); default 2.
    #[serde(default)]
    pub urgency: Option<i16>,
    /// `p1` to `p4`, instead of the priority matrix.
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub queue_id: Option<Uuid>,
    /// A contact at the organization.
    #[serde(default)]
    pub requester_id: Option<Uuid>,
    #[serde(default)]
    pub assignee_user_id: Option<Uuid>,
    #[serde(default)]
    pub assignee_team_id: Option<Uuid>,
    /// Where it came from (default `web`).
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::option_option)] // JSON's three states: missing, null, a value.
pub struct TicketChanges {
    /// The version you read.
    pub version: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub impact: Option<i16>,
    #[serde(default)]
    pub urgency: Option<i16>,
    /// `p1` to `p4`, or `auto` to follow the priority matrix again.
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub queue_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub requester_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub assignee_user_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub assignee_team_id: Option<Option<Uuid>>,
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<Ticket> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// Which tickets to list.
#[derive(Debug, Clone, Default, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TicketFilter {
    pub org_id: Option<Uuid>,
    pub queue_id: Option<Uuid>,
    /// A person's id, or `none` for unassigned.
    pub assignee: Option<String>,
    /// `open` (not resolved or closed, the default), `resolved`, `closed` or `all`.
    pub state: Option<String>,
    /// Matches titles and the ticket number.
    pub search: Option<String>,
    /// Only these priorities, comma-separated: e.g. `p1,p2`.
    pub priority: Option<String>,
    /// 1 to 200 (default 50).
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketPage {
    pub items: Vec<Ticket>,
    pub total: i64,
}

/// Tickets visible in the transaction's scope, most urgent first, then
/// oldest first.
pub async fn list(conn: &mut PgConnection, filter: &TicketFilter) -> DeskResult<TicketPage> {
    let categories: &[&str] = match filter.state.as_deref().unwrap_or("open") {
        "open" => &["new", "open", "pending"],
        "resolved" => &["resolved"],
        "closed" => &["closed"],
        "all" => &["new", "open", "pending", "resolved", "closed"],
        _ => return Err(DeskError::invalid("state", "open, resolved, closed or all")),
    };
    let (unassigned, assignee) = match filter.assignee.as_deref() {
        None => (false, None),
        Some("none") => (true, None),
        Some(id) => (
            false,
            Some(
                id.parse::<Uuid>()
                    .map_err(|_| DeskError::invalid("assignee", "a person's id, or none"))?,
            ),
        ),
    };
    let search = filter
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let number: Option<i64> = search
        .and_then(|s| s.rsplit('-').next())
        .and_then(|n| n.parse().ok());
    let pattern = search.map(|s| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let priorities: Option<Vec<String>> = match filter.priority.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(list) => Some(
            list.split(',')
                .map(|p| priority(p.trim().to_lowercase().as_str()))
                .collect::<DeskResult<_>>()?,
        ),
    };
    let filter_sql = "WHERE s.category = ANY($1) \
        AND ($2::uuid IS NULL OR t.org_id = $2) AND ($3::uuid IS NULL OR t.queue_id = $3) \
        AND (NOT $4 OR (t.assignee_user_id IS NULL AND t.assignee_team_id IS NULL)) \
        AND ($5::uuid IS NULL OR t.assignee_user_id = $5) \
        AND ($6::text IS NULL OR t.title ILIKE $6 OR t.number = $7) \
        AND ($8::text[] IS NULL OR t.priority = ANY($8))";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM sd_tickets t JOIN sd_statuses s ON s.id = t.status_id {filter_sql}"
    )))
    .bind(categories)
    .bind(filter.org_id)
    .bind(filter.queue_id)
    .bind(unassigned)
    .bind(assignee)
    .bind(&pattern)
    .bind(number)
    .bind(&priorities)
    .fetch_one(&mut *conn)
    .await?;
    let items = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} {filter_sql} ORDER BY t.priority, t.created_at LIMIT $9 OFFSET $10"
    )))
    .bind(categories)
    .bind(filter.org_id)
    .bind(filter.queue_id)
    .bind(unassigned)
    .bind(assignee)
    .bind(&pattern)
    .bind(number)
    .bind(&priorities)
    .bind(filter.limit.unwrap_or(50).clamp(1, 200))
    .bind(filter.offset.unwrap_or(0).max(0))
    .fetch_all(conn)
    .await?;
    Ok(TicketPage { items, total })
}

fn level(field: &str, value: Option<i16>) -> DeskResult<i16> {
    let value = value.unwrap_or(2);
    if (1..=3).contains(&value) {
        Ok(value)
    } else {
        Err(DeskError::invalid(field, "1 (high) to 3 (low)"))
    }
}

fn priority(value: &str) -> DeskResult<String> {
    if PRIORITIES.contains(&value) {
        Ok(value.to_owned())
    } else {
        Err(DeskError::invalid("priority", "p1 to p4"))
    }
}

/// Checks the references a ticket of `org_id` may hold, as the caller sees
/// them (in the transaction's scope).
async fn check_refs(
    conn: &mut PgConnection,
    org_id: Uuid,
    queue: Option<Uuid>,
    requester: Option<Uuid>,
    user: Option<Uuid>,
    team: Option<Uuid>,
) -> DeskResult<()> {
    if let Some(queue) = queue {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sd_queues WHERE id = $1 AND archived_at IS NULL)",
        )
        .bind(queue)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(DeskError::invalid("queueId", "there's no such queue"));
        }
    }
    if let Some(contact) = requester {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM crm_contacts c \
               JOIN crm_contact_companies l ON l.contact_id = c.id AND l.org_id = $2 \
              WHERE c.id = $1 AND c.archived_at IS NULL)",
        )
        .bind(contact)
        .bind(org_id)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(DeskError::invalid(
                "requesterId",
                "a contact at the ticket's organization",
            ));
        }
    }
    if let Some(user) = user {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND status = 'active' AND kind = 'human')",
        )
        .bind(user)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(DeskError::invalid(
                "assigneeUserId",
                "there's no such person",
            ));
        }
    }
    if let Some(team) = team {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM teams WHERE id = $1)")
            .bind(team)
            .fetch_one(&mut *conn)
            .await?;
        if !ok {
            return Err(DeskError::invalid("assigneeTeamId", "there's no such team"));
        }
    }
    Ok(())
}

/// Records a change: the ticket's activity, the audit log, and an event.
pub(crate) async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &Ticket,
    kind: &str,
    details: Json,
) -> DeskResult<()> {
    sqlx::query(
        "INSERT INTO sd_ticket_activity (id, ticket_id, org_id, actor_id, kind, details) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(huxley_core::new_id())
    .bind(ticket.id)
    .bind(ticket.org_id)
    .bind(ctx.actor.id())
    .bind(kind)
    .bind(&details)
    .execute(&mut *conn)
    .await?;
    let event = match kind {
        "created" => "ticket.created",
        "status" => "ticket.status_changed",
        "assigned" => "ticket.assigned",
        "note" => "ticket.note_added",
        _ => "ticket.updated",
    };
    audit::record(
        conn,
        ctx,
        AuditRecord::new(event)
            .org(ticket.org_id)
            .target("ticket", ticket.id)
            .details(json!({ "reference": ticket.reference, "change": details })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(ticket.org_id),
            subject_type: "ticket",
            subject_id: ticket.id,
            payload: json!({
                "reference": ticket.reference,
                "title": ticket.title,
                "status": ticket.status_key,
                "category": ticket.status_category,
                "priority": ticket.priority,
                "queueId": ticket.queue_id,
                "assigneeUserId": ticket.assignee_user_id,
                "assigneeTeamId": ticket.assignee_team_id,
                // Who made the change (none: the system or a requester).
                "actorId": ctx.actor.id(),
                "change": details,
            }),
        },
    )
    .await?;
    Ok(())
}

/// Creates a ticket in its type's first `new` status. `prefix` is the
/// organization's ticket prefix; `sla` the terms it's opened under.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &NewTicket,
    prefix: &str,
    sla: Option<&crate::sla::Plan>,
) -> DeskResult<Ticket> {
    let title = text("title", Some(&input.title), 300)?
        .ok_or_else(|| DeskError::invalid("title", "a ticket needs a title"))?;
    let description =
        text("description", input.description.as_deref(), 100_000)?.unwrap_or_default();
    let (type_id, _) =
        config::type_by_key(conn, input.r#type.as_deref().unwrap_or("incident")).await?;
    let status: Uuid = sqlx::query_scalar(
        "SELECT id FROM sd_statuses WHERE type_id = $1 AND category = 'new' ORDER BY position LIMIT 1",
    )
    .bind(type_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DeskError::invalid("type", "this type has no starting status"))?;
    let impact = level("impact", input.impact)?;
    let urgency = level("urgency", input.urgency)?;
    let (priority, overridden) = match &input.priority {
        Some(p) => (priority(p)?, true),
        None => (config::priority_for(conn, impact, urgency).await?, false),
    };
    let source = input.source.as_deref().unwrap_or("web");
    if !SOURCES.contains(&source) {
        return Err(DeskError::invalid("source", SOURCES.join(", ")));
    }
    check_refs(
        conn,
        input.org_id,
        input.queue_id,
        input.requester_id,
        input.assignee_user_id,
        input.assignee_team_id,
    )
    .await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_tickets (id, org_id, number_prefix, type_id, status_id, title, description, impact, \
                                 urgency, priority, priority_overridden, queue_id, requester_id, \
                                 assignee_user_id, assignee_team_id, source, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(id)
    .bind(input.org_id)
    .bind(prefix)
    .bind(type_id)
    .bind(status)
    .bind(&title)
    .bind(&description)
    .bind(impact)
    .bind(urgency)
    .bind(&priority)
    .bind(overridden)
    .bind(input.queue_id)
    .bind(input.requester_id)
    .bind(input.assignee_user_id)
    .bind(input.assignee_team_id)
    .bind(source)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if let Some(plan) = sla {
        crate::sla::start(conn, id, input.org_id, plan).await?;
    }
    let ticket = get(conn, id).await?;
    record(
        conn,
        ctx,
        &ticket,
        "created",
        json!({ "title": ticket.title, "source": source }),
    )
    .await?;
    Ok(ticket)
}

/// What changed between two versions of a ticket, field by field.
fn changes_between(before: &Ticket, after: &Ticket) -> serde_json::Map<String, Json> {
    let mut diff = serde_json::Map::new();
    let mut note = |field: &str, from: Json, to: Json| {
        if from != to {
            diff.insert(field.to_owned(), json!({ "from": from, "to": to }));
        }
    };
    note("title", json!(before.title), json!(after.title));
    note("impact", json!(before.impact), json!(after.impact));
    note("urgency", json!(before.urgency), json!(after.urgency));
    note("priority", json!(before.priority), json!(after.priority));
    note("queue", json!(before.queue_name), json!(after.queue_name));
    note(
        "requester",
        json!(before.requester_name),
        json!(after.requester_name),
    );
    note(
        "assignee",
        json!(before.assignee_user_name),
        json!(after.assignee_user_name),
    );
    note(
        "team",
        json!(before.assignee_team_name),
        json!(after.assignee_team_name),
    );
    if before.description != after.description {
        diff.insert("description".into(), json!({ "changed": true }));
    }
    diff
}

/// Changes a ticket's fields (not its status: see [`change_status`]).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    changes: &TicketChanges,
) -> DeskResult<Ticket> {
    let before = get(conn, id).await?;
    if before.version != changes.version {
        return Err(DeskError::VersionConflict);
    }
    let title = match &changes.title {
        Some(t) => text("title", Some(t), 300)?
            .ok_or_else(|| DeskError::invalid("title", "a ticket needs a title"))?,
        None => before.title.clone(),
    };
    let description = match &changes.description {
        Some(d) => text("description", Some(d), 100_000)?.unwrap_or_default(),
        None => before.description.clone(),
    };
    let impact = level("impact", Some(changes.impact.unwrap_or(before.impact)))?;
    let urgency = level("urgency", Some(changes.urgency.unwrap_or(before.urgency)))?;
    let (priority, overridden) = match changes.priority.as_deref() {
        Some("auto") => (config::priority_for(conn, impact, urgency).await?, false),
        Some(p) => (priority(p)?, true),
        None if before.priority_overridden => (before.priority.clone(), true),
        None => (config::priority_for(conn, impact, urgency).await?, false),
    };
    let queue = changes.queue_id.unwrap_or(before.queue_id);
    let requester = changes.requester_id.unwrap_or(before.requester_id);
    let user = changes.assignee_user_id.unwrap_or(before.assignee_user_id);
    let team = changes.assignee_team_id.unwrap_or(before.assignee_team_id);
    check_refs(
        conn,
        before.org_id,
        changes.queue_id.flatten(),
        changes.requester_id.flatten(),
        changes.assignee_user_id.flatten(),
        changes.assignee_team_id.flatten(),
    )
    .await?;
    let rows = sqlx::query(
        "UPDATE sd_tickets SET title = $3, description = $4, impact = $5, urgency = $6, priority = $7, \
                priority_overridden = $8, queue_id = $9, requester_id = $10, assignee_user_id = $11, \
                assignee_team_id = $12, version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(changes.version)
    .bind(&title)
    .bind(&description)
    .bind(impact)
    .bind(urgency)
    .bind(&priority)
    .bind(overridden)
    .bind(queue)
    .bind(requester)
    .bind(user)
    .bind(team)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    crate::sla::sync(conn, id).await?;
    let after = get(conn, id).await?;
    let diff = changes_between(&before, &after);
    if !diff.is_empty() {
        let reassigned = diff.contains_key("assignee") || diff.contains_key("team");
        let kind = if reassigned && diff.len() <= 2 {
            "assigned"
        } else {
            "updated"
        };
        record(conn, ctx, &after, kind, Json::Object(diff)).await?;
    }
    Ok(after)
}

/// The same changes for many tickets (bulk edit): each field left out is
/// unchanged, `null` clears.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::option_option)] // JSON's three states: missing, null, a value.
pub struct BulkFields {
    /// `p1` to `p4`, or `auto` to follow the priority matrix again.
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub queue_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub assignee_user_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>, nullable)]
    pub assignee_team_id: Option<Option<Uuid>>,
}

impl BulkFields {
    /// Whether anything changes.
    pub fn is_empty(&self) -> bool {
        self.priority.is_none()
            && self.queue_id.is_none()
            && self.assignee_user_id.is_none()
            && self.assignee_team_id.is_none()
    }

    /// These changes to a ticket as it is (`version`).
    pub fn at(&self, version: i32) -> TicketChanges {
        TicketChanges {
            version,
            priority: self.priority.clone(),
            queue_id: self.queue_id,
            assignee_user_id: self.assignee_user_id,
            assignee_team_id: self.assignee_team_id,
            ..TicketChanges::default()
        }
    }
}

/// Notes on a ticket that an Action (a form) was run from it, and the runs
/// it started.
pub async fn record_action(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    form: &str,
    runs: &[Uuid],
) -> DeskResult<()> {
    let t = get(conn, ticket).await?;
    record(
        conn,
        ctx,
        &t,
        "action",
        json!({ "form": form, "runs": runs }),
    )
    .await
}

/// A status change.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusChange {
    /// The new status's key.
    pub status: String,
    /// A resolution note, for statuses that need one.
    #[serde(default)]
    pub resolution: Option<String>,
    /// The version you read.
    pub version: i32,
}

/// Moves a ticket to another status of its type: allowed changes only,
/// with what the status needs to enter it.
pub async fn change_status(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    change: &StatusChange,
) -> DeskResult<Ticket> {
    let before = get(conn, id).await?;
    if before.version != change.version {
        return Err(DeskError::VersionConflict);
    }
    let (type_id, from): (Uuid, Uuid) =
        sqlx::query_as("SELECT type_id, status_id FROM sd_tickets WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    let target = config::statuses(conn, type_id)
        .await?
        .into_iter()
        .find(|s| s.key == change.status)
        .ok_or_else(|| {
            DeskError::invalid(
                "status",
                format!("{:?} isn't a status of {}", change.status, before.type_name),
            )
        })?;
    if !config::may_change(conn, type_id, from, target.id).await? {
        return Err(DeskError::invalid(
            "status",
            format!(
                "a {} can't go from {} to {}",
                before.type_name, before.status_name, target.name
            ),
        ));
    }
    let resolution = text("resolution", change.resolution.as_deref(), 100_000)?;
    for need in &target.requires {
        match need.as_str() {
            "resolution" => {
                let has: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM sd_ticket_notes WHERE ticket_id = $1 AND kind = 'resolution')",
                )
                .bind(id)
                .fetch_one(&mut *conn)
                .await?;
                if !has && resolution.is_none() {
                    return Err(DeskError::invalid(
                        "resolution",
                        format!("{} needs a resolution note", target.name),
                    ));
                }
            }
            "assignee"
                if before.assignee_user_id.is_none() && before.assignee_team_id.is_none() =>
            {
                return Err(DeskError::invalid(
                    "status",
                    format!("{} needs the ticket assigned", target.name),
                ));
            }
            _ => {}
        }
    }
    if let Some(body) = &resolution {
        insert_note(conn, &before, ctx, "resolution", body).await?;
    }
    let rows = sqlx::query(
        "UPDATE sd_tickets SET status_id = $3, version = version + 1, updated_at = now(), \
                resolved_at = CASE WHEN $4 IN ('resolved', 'closed') THEN coalesce(resolved_at, now()) END, \
                closed_at = CASE WHEN $4 = 'closed' THEN coalesce(closed_at, now()) END \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(change.version)
    .bind(target.id)
    .bind(&target.category)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    crate::sla::sync(conn, id).await?;
    let after = get(conn, id).await?;
    record(
        conn,
        ctx,
        &after,
        "status",
        json!({ "from": before.status_key, "to": after.status_key, "category": after.status_category }),
    )
    .await?;
    Ok(after)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: Uuid,
    /// `public`, `internal` or `resolution`.
    pub kind: String,
    pub body: String,
    pub author_user_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub author_contact_id: Option<Uuid>,
    /// The contact who wrote it (by email).
    pub author_contact_name: Option<String>,
    /// The address it came from, when it came by email.
    pub author_email: Option<String>,
    /// The email it came in, if it did.
    pub email_id: Option<Uuid>,
    /// Who it was emailed to (public notes, with the service desk's email
    /// workflows).
    pub emailed_to: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub emailed_at: Option<OffsetDateTime>,
    /// Why emailing it failed, if it did.
    pub email_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewNote {
    /// `public` (the requester can see it), `internal` or `resolution`.
    pub kind: String,
    pub body: String,
}

async fn insert_note(
    conn: &mut PgConnection,
    ticket: &Ticket,
    ctx: &RequestContext,
    kind: &str,
    body: &str,
) -> DeskResult<Uuid> {
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_ticket_notes (id, ticket_id, org_id, kind, body, author_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(ticket.id)
    .bind(ticket.org_id)
    .bind(kind)
    .bind(body)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

/// The ticket prefix for an organization (`numbering.ticket_prefix`;
/// system scope, since it's inherited from above).
pub async fn prefix_for(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<String> {
    let settings = huxley_orgs::settings::resolve(conn, org_id)
        .await
        .map_err(|e| DeskError::invalid("orgId", e.to_string()))?;
    Ok(settings
        .into_iter()
        .find(|s| s.def.key == huxley_orgs::settings::TICKET_PREFIX)
        .and_then(|s| s.value)
        .unwrap_or_else(|| "T".into()))
}

/// Adds a public note that came by email from `from` (a contact, if
/// they're one). It doesn't count as a first response.
pub async fn add_email_note(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &Ticket,
    from: &str,
    contact: Option<Uuid>,
    body: &str,
    email_id: Uuid,
) -> DeskResult<Uuid> {
    let body = text("body", Some(body), 100_000)?.unwrap_or_else(|| "(no text)".into());
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_ticket_notes (id, ticket_id, org_id, kind, body, author_contact_id, author_email, email_id) \
         VALUES ($1, $2, $3, 'public', $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(ticket.id)
    .bind(ticket.org_id)
    .bind(&body)
    .bind(contact)
    .bind(from)
    .bind(email_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sd_tickets SET updated_at = now() WHERE id = $1")
        .bind(ticket.id)
        .execute(&mut *conn)
        .await?;
    crate::sla::sync(conn, ticket.id).await?;
    record(
        conn,
        ctx,
        ticket,
        "note",
        json!({ "kind": "public", "note": id, "email": email_id, "from": from }),
    )
    .await?;
    Ok(id)
}

/// Adds a contact's public note (from the portal, say), with `details`
/// recorded on the activity and the `ticket.note_added` event.
pub async fn add_contact_note(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &Ticket,
    contact: Uuid,
    body: &str,
    details: serde_json::Value,
) -> DeskResult<Uuid> {
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_ticket_notes (id, ticket_id, org_id, kind, body, author_contact_id) \
         VALUES ($1, $2, $3, 'public', $4, $5)",
    )
    .bind(id)
    .bind(ticket.id)
    .bind(ticket.org_id)
    .bind(body)
    .bind(contact)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE sd_tickets SET updated_at = now() WHERE id = $1")
        .bind(ticket.id)
        .execute(&mut *conn)
        .await?;
    crate::sla::sync(conn, ticket.id).await?;
    let mut change = json!({ "kind": "public", "note": id });
    if let (Some(c), Some(extra)) = (change.as_object_mut(), details.as_object()) {
        c.extend(extra.clone());
    }
    record(conn, ctx, ticket, "note", change).await?;
    Ok(id)
}

/// Adds a note. The first public note by a person sets the first response.
pub async fn add_note(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    note: &NewNote,
) -> DeskResult<Note> {
    if !NOTE_KINDS.contains(&note.kind.as_str()) {
        return Err(DeskError::invalid("kind", "public, internal or resolution"));
    }
    let body = text("body", Some(&note.body), 100_000)?
        .ok_or_else(|| DeskError::invalid("body", "a note needs text"))?;
    let ticket = get(conn, id).await?;
    let note_id = insert_note(conn, &ticket, ctx, &note.kind, &body).await?;
    if note.kind == "public" && ctx.actor.id().is_some() {
        sqlx::query(
            "UPDATE sd_tickets SET first_response_at = coalesce(first_response_at, now()), updated_at = now() \
              WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?;
    } else {
        sqlx::query("UPDATE sd_tickets SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    crate::sla::sync(conn, id).await?;
    record(
        conn,
        ctx,
        &ticket,
        "note",
        json!({ "kind": note.kind, "note": note_id }),
    )
    .await?;
    notes(conn, id)
        .await?
        .into_iter()
        .find(|n| n.id == note_id)
        .ok_or(DeskError::NotFound)
}

/// A ticket's notes, oldest first.
pub async fn notes(conn: &mut PgConnection, id: Uuid) -> DeskResult<Vec<Note>> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.kind, n.body, n.author_user_id, u.display_name AS author_name, n.author_contact_id, \
                nullif(btrim(c.first_name || ' ' || c.last_name), '') AS author_contact_name, \
                n.author_email, n.email_id, n.emailed_to, n.emailed_at, n.email_error, n.created_at \
           FROM sd_ticket_notes n LEFT JOIN users u ON u.id = n.author_user_id \
           LEFT JOIN crm_contacts c ON c.id = n.author_contact_id \
          WHERE n.ticket_id = $1 ORDER BY n.created_at, n.id",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: Uuid,
    /// `created`, `updated`, `status`, `assigned` or `note`.
    pub kind: String,
    #[schema(value_type = Object)]
    pub details: Json,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A ticket's activity, oldest first.
pub async fn activity(conn: &mut PgConnection, id: Uuid) -> DeskResult<Vec<Activity>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.kind, a.details, a.actor_id, u.display_name AS actor_name, a.at \
           FROM sd_ticket_activity a LEFT JOIN users u ON u.id = a.actor_id \
          WHERE a.ticket_id = $1 ORDER BY a.at, a.id",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}
