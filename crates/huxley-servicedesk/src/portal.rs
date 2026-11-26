//! Tickets as the client portal sees them (ADR-040): a company's tickets, or
//! a contact's own; public notes only; no internal notes, assignees, time,
//! queues or SLA internals. Contacts raise tickets and reply to them.

use huxley_core::RequestContext;
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{self, NewTicket};
use crate::{DeskError, DeskResult, text};

/// A ticket in the portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalTicket {
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub description: String,
    pub status_name: String,
    /// `new`, `open`, `pending`, `resolved` or `closed`.
    pub status_category: String,
    pub priority: String,
    pub requester_id: Option<Uuid>,
    pub requester_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A public note in the portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalNote {
    pub id: Uuid,
    pub body: String,
    /// Who wrote it: a contact, or someone at the service desk.
    pub author_name: Option<String>,
    /// Written by the service desk (not a contact).
    pub from_support: bool,
    /// The resolution.
    pub resolution: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Which tickets a portal account sees.
#[derive(Debug, Clone, Copy)]
pub enum Seen {
    /// Every ticket of the company.
    Company,
    /// Only those this contact asked for.
    Own(Uuid),
}

const SELECT: &str = "SELECT t.id, t.number_prefix || '-' || t.number AS reference, t.title, t.description, \
    s.name AS status_name, s.category AS status_category, t.priority, t.requester_id, \
    NULLIF(btrim(c.first_name || ' ' || c.last_name), '') AS requester_name, t.created_at, t.updated_at \
    FROM sd_tickets t JOIN sd_statuses s ON s.id = t.status_id \
    LEFT JOIN crm_contacts c ON c.id = t.requester_id";

/// A company's tickets (or a contact's own), newest activity first. `open`
/// keeps those not yet resolved; otherwise resolved and closed ones.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Uuid,
    seen: Seen,
    open: bool,
    q: Option<&str>,
) -> DeskResult<Vec<PortalTicket>> {
    let own = match seen {
        Seen::Company => None,
        Seen::Own(contact) => Some(contact),
    };
    let q = q.map(str::trim).filter(|q| !q.is_empty());
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.org_id = $1 AND ($2::uuid IS NULL OR t.requester_id = $2) \
           AND (s.category NOT IN ('resolved', 'closed')) = $3 \
           AND ($4::text IS NULL OR t.title ILIKE '%' || $4 || '%' \
                OR (t.number_prefix || '-' || t.number) ILIKE $4) \
         ORDER BY t.updated_at DESC LIMIT 200"
    )))
    .bind(org_id)
    .bind(own)
    .bind(open)
    .bind(q)
    .fetch_all(conn)
    .await?)
}

/// One ticket, if the account sees it (else not found).
pub async fn get(
    conn: &mut PgConnection,
    org_id: Uuid,
    seen: Seen,
    id: Uuid,
) -> DeskResult<PortalTicket> {
    let ticket: PortalTicket = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE t.id = $1 AND t.org_id = $2"
    )))
    .bind(id)
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(DeskError::NotFound)?;
    match seen {
        Seen::Own(contact) if ticket.requester_id != Some(contact) => Err(DeskError::NotFound),
        _ => Ok(ticket),
    }
}

/// A ticket's public notes and resolution, oldest first.
pub async fn notes(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Vec<PortalNote>> {
    Ok(sqlx::query_as(
        "SELECT n.id, n.body, \
                COALESCE(NULLIF(btrim(c.first_name || ' ' || c.last_name), ''), u.display_name) AS author_name, \
                n.author_contact_id IS NULL AS from_support, n.kind = 'resolution' AS resolution, n.created_at \
           FROM sd_ticket_notes n LEFT JOIN users u ON u.id = n.author_user_id \
           LEFT JOIN crm_contacts c ON c.id = n.author_contact_id \
          WHERE n.ticket_id = $1 AND n.kind IN ('public', 'resolution') \
          ORDER BY n.created_at, n.id",
    )
    .bind(ticket)
    .fetch_all(conn)
    .await?)
}

/// Raises a ticket for a contact at their company (the default type and
/// routing, source `portal`).
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    contact: Uuid,
    title: &str,
    description: &str,
) -> DeskResult<PortalTicket> {
    let title = text("title", Some(title), 300)?
        .ok_or_else(|| DeskError::invalid("title", "say what you need"))?;
    let input = NewTicket {
        org_id,
        title,
        description: text("description", Some(description), 100_000)?,
        requester_id: Some(contact),
        source: Some("portal".into()),
        ..Default::default()
    };
    let prefix = tickets::prefix_for(conn, org_id).await?;
    let plan = crate::sla::plan_for(conn, org_id).await?;
    let ticket = tickets::create(conn, ctx, &input, &prefix, plan.as_ref()).await?;
    get(conn, org_id, Seen::Company, ticket.id).await
}

/// A contact's reply: a public note by them. A resolved ticket reopens.
pub async fn reply(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &PortalTicket,
    contact: Uuid,
    body: &str,
) -> DeskResult<PortalNote> {
    let body = text("body", Some(body), 100_000)?
        .ok_or_else(|| DeskError::invalid("body", "write a reply"))?;
    if ticket.status_category == "closed" {
        return Err(DeskError::invalid(
            "body",
            "this ticket is closed; raise a new one",
        ));
    }
    if ticket.status_category == "resolved" {
        let full = tickets::get(conn, ticket.id).await?;
        crate::mail::reopen(conn, ctx, &full).await?;
    }
    let full = tickets::get(conn, ticket.id).await?;
    let id = tickets::add_contact_note(
        conn,
        ctx,
        &full,
        contact,
        &body,
        json!({ "portal": true, "contact": contact }),
    )
    .await?;
    notes(conn, ticket.id)
        .await?
        .into_iter()
        .find(|n| n.id == id)
        .ok_or(DeskError::NotFound)
}
