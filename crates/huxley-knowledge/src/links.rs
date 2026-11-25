//! Articles linked to tickets, and articles suggested for a ticket.
//!
//! A link belongs to the ticket's organization; the article may be the
//! MSP's or the client's. What's listed is what the transaction's scope can
//! see of both. Suggestions are published articles whose words best match
//! the ticket's title and description.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::articles::{self, Drafts};
use crate::{KbError, KbResult};

/// An article linked to (or suggested for) a ticket.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbTicketArticle)]
pub struct TicketArticle {
    pub id: Uuid,
    pub title: String,
    pub status: String,
    pub org_name: String,
    pub category_name: Option<String>,
    /// Who linked it, and when (none for a suggestion).
    pub linked_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub linked_at: Option<OffsetDateTime>,
    /// Where a suggestion matched, with the matched words in `<b>`.
    pub snippet: Option<String>,
}

/// A ticket an article is linked to.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbLinkedTicket)]
pub struct LinkedTicket {
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub org_name: String,
    pub status_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub linked_at: OffsetDateTime,
}

fn drafts(d: &Drafts) -> (bool, Vec<Uuid>) {
    match d {
        Drafts::All => (true, Vec::new()),
        Drafts::None => (false, Vec::new()),
        Drafts::In(orgs) => (false, orgs.clone()),
    }
}

/// The articles linked to a ticket that the scope can see: drafts where
/// `drafts` allows, else only published ones, by their published title.
pub async fn for_ticket(
    conn: &mut PgConnection,
    ticket: Uuid,
    drafts_in: &Drafts,
) -> KbResult<Vec<TicketArticle>> {
    let (all, orgs) = drafts(drafts_in);
    Ok(sqlx::query_as(
        "SELECT a.id, \
                CASE WHEN $2 OR a.org_id = ANY($3) THEN a.title ELSE r.title END AS title, \
                CASE WHEN $2 OR a.org_id = ANY($3) THEN a.status ELSE 'published' END AS status, \
                o.name AS org_name, c.name AS category_name, u.display_name AS linked_by_name, \
                l.created_at AS linked_at, NULL::text AS snippet \
           FROM kb_article_tickets l \
           JOIN kb_articles a ON a.id = l.article_id \
           JOIN organizations o ON o.id = a.org_id \
           LEFT JOIN kb_categories c ON c.id = a.category_id \
           LEFT JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
           LEFT JOIN users u ON u.id = l.linked_by \
          WHERE l.ticket_id = $1 \
            AND ($2 OR a.org_id = ANY($3) OR (a.published_revision IS NOT NULL AND a.status <> 'archived')) \
          ORDER BY l.created_at",
    )
    .bind(ticket)
    .bind(all)
    .bind(&orgs)
    .fetch_all(conn)
    .await?)
}

/// Up to `limit` published articles best matching a ticket's `text` (its
/// title and description: any of its most frequent words), leaving out
/// those already linked to `ticket`.
pub async fn suggest(
    conn: &mut PgConnection,
    ticket: Uuid,
    text: &str,
    limit: i64,
) -> KbResult<Vec<TicketArticle>> {
    Ok(sqlx::query_as(
        "WITH t AS ( \
            SELECT nullif(( \
                SELECT string_agg(quote_literal(lexeme) || ':*', ' | ') FROM ( \
                    SELECT lexeme FROM unnest(to_tsvector('english', left($3, 6000))) \
                     ORDER BY cardinality(positions) DESC LIMIT 40) w \
            ), '')::tsquery AS q) \
         SELECT a.id, r.title, 'published' AS status, o.name AS org_name, c.name AS category_name, \
                NULL::text AS linked_by_name, NULL::timestamptz AS linked_at, \
                ts_headline('english', r.body, t.q, \
                    'StartSel=<b>, StopSel=</b>, MaxWords=25, MinWords=10, MaxFragments=1') AS snippet \
           FROM t \
           JOIN kb_articles a ON a.search @@ t.q \
           JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
           JOIN organizations o ON o.id = a.org_id \
           LEFT JOIN kb_categories c ON c.id = a.category_id \
          WHERE a.status <> 'archived' AND a.published_revision IS NOT NULL \
            AND NOT EXISTS (SELECT 1 FROM kb_article_tickets l WHERE l.article_id = a.id AND l.ticket_id = $1) \
          ORDER BY ts_rank(a.search, t.q) DESC, a.published_at DESC \
          LIMIT $2",
    )
    .bind(ticket)
    .bind(limit.clamp(1, 20))
    .bind(text)
    .fetch_all(conn)
    .await?)
}

/// The tickets an article is linked to that the scope can see, newest link
/// first.
pub async fn tickets_for_article(
    conn: &mut PgConnection,
    article: Uuid,
) -> KbResult<Vec<LinkedTicket>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.number_prefix || '-' || t.number AS reference, t.title, o.name AS org_name, \
                s.name AS status_name, l.created_at AS linked_at \
           FROM kb_article_tickets l \
           JOIN sd_tickets t ON t.id = l.ticket_id \
           JOIN organizations o ON o.id = t.org_id \
           JOIN sd_statuses s ON s.id = t.status_id \
          WHERE l.article_id = $1 ORDER BY l.created_at DESC LIMIT 200",
    )
    .bind(article)
    .fetch_all(conn)
    .await?)
}

/// The ticket's organization and reference (system scope).
async fn ticket(conn: &mut PgConnection, id: Uuid) -> KbResult<(Uuid, String)> {
    sqlx::query_as("SELECT org_id, number_prefix || '-' || number FROM sd_tickets WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(KbError::NotFound)
}

/// Links an article to a ticket (again is fine). Archived articles can't
/// be linked.
pub async fn link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    article: Uuid,
    ticket_id: Uuid,
) -> KbResult<()> {
    let a = articles::get(conn, article).await?;
    if a.summary.status == "archived" {
        return Err(KbError::State("an archived article can't be linked".into()));
    }
    let (org, reference) = ticket(conn, ticket_id).await?;
    let added = sqlx::query(
        "INSERT INTO kb_article_tickets (article_id, ticket_id, org_id, linked_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(article)
    .bind(ticket_id)
    .bind(org)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if added == 0 {
        return Ok(());
    }
    announce(
        conn,
        ctx,
        &a,
        "kb.article_linked",
        org,
        ticket_id,
        &reference,
    )
    .await
}

/// Removes a link.
pub async fn unlink(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    article: Uuid,
    ticket_id: Uuid,
) -> KbResult<()> {
    let a = articles::get(conn, article).await?;
    let (org, reference) = ticket(conn, ticket_id).await?;
    let removed =
        sqlx::query("DELETE FROM kb_article_tickets WHERE article_id = $1 AND ticket_id = $2")
            .bind(article)
            .bind(ticket_id)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    if removed == 0 {
        return Err(KbError::NotFound);
    }
    announce(
        conn,
        ctx,
        &a,
        "kb.article_unlinked",
        org,
        ticket_id,
        &reference,
    )
    .await
}

async fn announce(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    a: &articles::Article,
    event: &'static str,
    ticket_org: Uuid,
    ticket_id: Uuid,
    reference: &str,
) -> KbResult<()> {
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(ticket_org)
            .target("kb_article", a.summary.id)
            .details(
                json!({ "title": a.summary.title, "ticketId": ticket_id, "reference": reference }),
            ),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(ticket_org),
            subject_type: "kb_article",
            subject_id: a.summary.id,
            payload: json!({
                "title": a.summary.title,
                "ticketId": ticket_id,
                "reference": reference,
                "actorId": ctx.actor.id(),
            }),
        },
    )
    .await?;
    Ok(())
}
