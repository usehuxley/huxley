//! Published articles embedded for search by meaning (Phase 6, building on
//! the Phase 5 ticket embeddings): each article's published title and
//! text, by its organization's embeddings provider. pgvector is optional:
//! embeddings are kept as real[] either way, and searching (and so
//! embedding) needs it.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::KbResult;

/// Characters of an article sent to be embedded.
pub const MAX_TEXT: usize = 8000;

/// Whether pgvector is installed.
pub async fn available(conn: &mut PgConnection) -> KbResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(conn)
            .await?,
    )
}

/// A published article whose embedding is missing or out of date.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Stale {
    pub article_id: Uuid,
    pub org_id: Uuid,
    /// The published title, then text.
    pub text: String,
    pub content_hash: String,
    /// The model its organization's embeddings provider uses now.
    pub model: String,
}

/// Up to `limit` published articles at `root` and below, most recently
/// published first, whose embedding is missing or stale. Only those whose
/// organization has an embeddings provider. System scope.
pub async fn stale(conn: &mut PgConnection, root: Uuid, limit: i64) -> KbResult<Vec<Stale>> {
    Ok(sqlx::query_as(
        "SELECT a.id AS article_id, a.org_id, \
                left(r.title || E'\\n\\n' || r.body, $3) AS text, \
                md5(r.title || E'\\n' || r.body) AS content_hash, pm.model \
           FROM kb_articles a \
           JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
           JOIN organizations o ON o.id = a.org_id \
          CROSS JOIN LATERAL ( \
                SELECT p.model FROM ai_providers p JOIN organizations po ON po.id = p.org_id \
                 WHERE p.purpose = 'embeddings' AND po.path @> o.path \
                 ORDER BY nlevel(po.path) DESC LIMIT 1) pm \
           LEFT JOIN kb_article_embeddings e ON e.article_id = a.id \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) \
            AND a.status <> 'archived' \
            AND (e.article_id IS NULL OR e.model <> pm.model OR e.org_id <> a.org_id \
                 OR e.content_hash <> md5(r.title || E'\\n' || r.body)) \
          ORDER BY a.published_at DESC NULLS LAST \
          LIMIT $2",
    )
    .bind(root)
    .bind(limit)
    .bind(i32::try_from(MAX_TEXT).unwrap_or(8000))
    .fetch_all(conn)
    .await?)
}

/// Keeps an article's embedding (replacing any before).
pub async fn save(conn: &mut PgConnection, article: &Stale, vector: &[f32]) -> KbResult<()> {
    sqlx::query(
        "INSERT INTO kb_article_embeddings (article_id, org_id, model, content_hash, embedding) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (article_id) DO UPDATE SET org_id = $2, model = $3, content_hash = $4, \
                embedding = $5, embedded_at = now()",
    )
    .bind(article.article_id)
    .bind(article.org_id)
    .bind(&article.model)
    .bind(&article.content_hash)
    .bind(vector)
    .execute(conn)
    .await?;
    Ok(())
}

/// A published article found by meaning.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbMeaningHit)]
pub struct MeaningHit {
    pub id: Uuid,
    /// The published title.
    pub title: String,
    pub org_name: String,
    pub category_name: Option<String>,
    /// How close in meaning, from 0 to 1 (cosine similarity).
    pub score: f64,
}

const HIT: &str = "a.id, r.title, o.name AS org_name, c.name AS category_name";
const FROM: &str = "FROM kb_article_embeddings e \
    JOIN kb_articles a ON a.id = e.article_id \
    JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
    JOIN organizations o ON o.id = a.org_id \
    LEFT JOIN kb_categories c ON c.id = a.category_id";

/// Published articles embedded by `model` nearest in meaning to `vector`,
/// best first: what the transaction's scope can see. Needs pgvector.
pub async fn search(
    conn: &mut PgConnection,
    model: &str,
    vector: &[f32],
    limit: i64,
) -> KbResult<Vec<MeaningHit>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {HIT}, (1 - (e.embedding::vector <=> $2::real[]::vector))::float8 AS score {FROM} \
          WHERE e.model = $1 AND cardinality(e.embedding) = cardinality($2::real[]) AND a.status <> 'archived' \
          ORDER BY e.embedding::vector <=> $2::real[]::vector \
          LIMIT $3"
    )))
    .bind(model)
    .bind(vector)
    .bind(limit.clamp(1, 50))
    .fetch_all(conn)
    .await?)
}

/// Published articles nearest in meaning to a ticket, by the ticket's own
/// embedding (no AI call), leaving out those linked to it. Empty when the
/// ticket isn't embedded. Needs pgvector.
pub async fn near_ticket(
    conn: &mut PgConnection,
    ticket: Uuid,
    limit: i64,
) -> KbResult<Vec<MeaningHit>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {HIT}, (1 - (e.embedding::vector <=> te.embedding::vector))::float8 AS score {FROM} \
           JOIN sd_ticket_embeddings te ON te.ticket_id = $1 AND te.model = e.model \
                AND cardinality(te.embedding) = cardinality(e.embedding) \
          WHERE a.status <> 'archived' \
            AND NOT EXISTS (SELECT 1 FROM kb_article_tickets l WHERE l.article_id = a.id AND l.ticket_id = $1) \
          ORDER BY e.embedding::vector <=> te.embedding::vector \
          LIMIT $2"
    )))
    .bind(ticket)
    .bind(limit.clamp(1, 20))
    .fetch_all(conn)
    .await?)
}
