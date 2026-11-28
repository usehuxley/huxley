//! Help articles in the client portal (ADR-040): published articles meant
//! for clients (`clients` or `public`), of the contact's company and the
//! organizations above it. Only the published revision is shown.

use serde::Serialize;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{KbError, KbResult, markdown};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalArticleSummary)]
pub struct Summary {
    pub id: Uuid,
    pub title: String,
    pub category: Option<String>,
    /// Where a search matched, with the matched words in `<b>`.
    pub snippet: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalArticle)]
pub struct Article {
    pub id: Uuid,
    pub title: String,
    pub category: Option<String>,
    /// Rendered from markdown, safe to show.
    pub body_html: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
}

/// A category with articles for the company's contacts.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalArticleCategory)]
pub struct Category {
    pub id: Uuid,
    pub name: String,
    pub articles: i64,
}

const FROM: &str = "FROM kb_articles a JOIN organizations o ON o.id = a.org_id \
    JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
    LEFT JOIN kb_categories c ON c.id = a.category_id";

/// What the company's contacts may read.
const VISIBLE: &str = "a.published_revision IS NOT NULL AND a.status <> 'archived' \
    AND a.visibility IN ('clients', 'public') \
    AND (SELECT path FROM organizations WHERE id = $1) <@ o.path";

/// Articles for a company's contacts: by best match when searching, else
/// newest first.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Uuid,
    q: Option<&str>,
    category: Option<Uuid>,
) -> KbResult<Vec<Summary>> {
    let q = q.map(str::trim).filter(|q| !q.is_empty());
    let query = "websearch_to_tsquery('english', $2)";
    let document = "to_tsvector('english', r.title || ' ' || r.body)";
    let headline = crate::highlight::options(30, 12);
    let rows: Vec<Summary> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.id, r.title, c.name AS category, a.published_at, \
                CASE WHEN $2::text IS NULL THEN NULL ELSE ts_headline('english', r.body, {query}, \
                     '{headline}') END AS snippet \
           {FROM} WHERE {VISIBLE} \
            AND ($2::text IS NULL OR {document} @@ {query} OR r.title ILIKE '%' || $2 || '%') \
            AND ($3::uuid IS NULL OR a.category_id = $3) \
          ORDER BY CASE WHEN $2::text IS NULL THEN 0 ELSE ts_rank({document}, {query}) END DESC, \
                   a.published_at DESC NULLS LAST \
          LIMIT 100"
    )))
    .bind(org_id)
    .bind(q)
    .bind(category)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|mut r| {
            r.snippet = r.snippet.as_deref().map(crate::highlight::to_html);
            r
        })
        .collect())
}

/// The categories with articles for a company's contacts.
pub async fn categories(conn: &mut PgConnection, org_id: Uuid) -> KbResult<Vec<Category>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT c.id, c.name, count(*) AS articles {FROM} WHERE {VISIBLE} AND c.id IS NOT NULL \
          GROUP BY c.id, c.name ORDER BY lower(c.name)"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

/// One article, if the company's contacts may read it.
pub async fn get(conn: &mut PgConnection, org_id: Uuid, id: Uuid) -> KbResult<Article> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        title: String,
        category: Option<String>,
        body: String,
        published_at: Option<OffsetDateTime>,
    }
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.id, r.title, c.name AS category, r.body, a.published_at {FROM} WHERE {VISIBLE} AND a.id = $2"
    )))
    .bind(org_id)
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(KbError::NotFound)?;
    Ok(Article {
        id: row.id,
        title: row.title,
        category: row.category,
        body_html: markdown::render(&row.body),
        published_at: row.published_at,
    })
}
