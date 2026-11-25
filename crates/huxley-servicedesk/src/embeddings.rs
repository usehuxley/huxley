//! Tickets embedded for search by meaning (Phase 5): each ticket's title
//! and description, by its organization's embeddings provider. pgvector is
//! optional: embeddings are kept as real[] either way, and searching (and so
//! embedding) needs it.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::DeskResult;

/// Characters of a ticket sent to be embedded (well under any provider's
/// token limit).
pub const MAX_TEXT: usize = 8000;

/// Whether pgvector is installed, so tickets can be searched by meaning.
pub async fn available(conn: &mut PgConnection) -> DeskResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(conn)
            .await?,
    )
}

/// A ticket whose embedding is missing or out of date.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Stale {
    pub ticket_id: Uuid,
    pub org_id: Uuid,
    /// What to embed: the title, then the description.
    pub text: String,
    pub content_hash: String,
    /// The model its organization's embeddings provider uses now.
    pub model: String,
}

/// Up to `limit` tickets at `root` and below, most recently changed first,
/// whose embedding is missing or stale: their text changed, they moved
/// organization, or their provider's model changed. Only tickets whose
/// organization has an embeddings provider (its own or above). System scope.
pub async fn stale(conn: &mut PgConnection, root: Uuid, limit: i64) -> DeskResult<Vec<Stale>> {
    Ok(sqlx::query_as(
        "SELECT t.id AS ticket_id, t.org_id, \
                left(t.title || E'\\n\\n' || t.description, $3) AS text, \
                md5(t.title || E'\\n' || t.description) AS content_hash, pm.model \
           FROM sd_tickets t \
           JOIN organizations o ON o.id = t.org_id \
          CROSS JOIN LATERAL ( \
                SELECT p.model FROM ai_providers p JOIN organizations po ON po.id = p.org_id \
                 WHERE p.purpose = 'embeddings' AND po.path @> o.path \
                 ORDER BY nlevel(po.path) DESC LIMIT 1) pm \
           LEFT JOIN sd_ticket_embeddings e ON e.ticket_id = t.id \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) \
            AND t.merged_into_id IS NULL \
            AND (e.ticket_id IS NULL OR e.model <> pm.model OR e.org_id <> t.org_id \
                 OR e.content_hash <> md5(t.title || E'\\n' || t.description)) \
          ORDER BY t.updated_at DESC \
          LIMIT $2",
    )
    .bind(root)
    .bind(limit)
    .bind(i32::try_from(MAX_TEXT).unwrap_or(8000))
    .fetch_all(conn)
    .await?)
}

/// Keeps a ticket's embedding (replacing any before).
pub async fn save(
    conn: &mut PgConnection,
    ticket: &Stale,
    model: &str,
    vector: &[f32],
) -> DeskResult<()> {
    sqlx::query(
        "INSERT INTO sd_ticket_embeddings (ticket_id, org_id, model, content_hash, embedding) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (ticket_id) DO UPDATE SET org_id = $2, model = $3, content_hash = $4, \
                embedding = $5, embedded_at = now()",
    )
    .bind(ticket.ticket_id)
    .bind(ticket.org_id)
    .bind(model)
    .bind(&ticket.content_hash)
    .bind(vector)
    .execute(conn)
    .await?;
    Ok(())
}

/// A ticket found by meaning.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeaningHit {
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub org_name: String,
    pub status_name: String,
    /// How close in meaning, from 0 to 1 (cosine similarity).
    pub score: f64,
}

/// Tickets embedded by `model` nearest in meaning to `vector`, best first:
/// what the transaction's scope can see. Needs pgvector ([`available`]).
pub async fn search(
    conn: &mut PgConnection,
    model: &str,
    vector: &[f32],
    limit: i64,
) -> DeskResult<Vec<MeaningHit>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.number_prefix || '-' || t.number AS reference, t.title, o.name AS org_name, \
                s.name AS status_name, \
                (1 - (e.embedding::vector <=> $2::real[]::vector))::float8 AS score \
           FROM sd_ticket_embeddings e \
           JOIN sd_tickets t ON t.id = e.ticket_id \
           JOIN organizations o ON o.id = t.org_id \
           JOIN sd_statuses s ON s.id = t.status_id \
          WHERE e.model = $1 AND cardinality(e.embedding) = cardinality($2::real[]) \
          ORDER BY e.embedding::vector <=> $2::real[]::vector \
          LIMIT $3",
    )
    .bind(model)
    .bind(vector)
    .bind(limit.clamp(1, 50))
    .fetch_all(conn)
    .await?)
}
