//! Articles: a working copy with revisions, and a review flow.
//!
//! - **draft**: being written. Editing a published article makes it a
//!   draft again, while readers keep seeing the published revision.
//! - **`in_review`**: submitted; someone who may publish approves (it's
//!   published) or rejects it with a comment (back to draft).
//! - **published**: readers see `published_revision`.
//! - **archived**: hidden from readers; restoring brings back what was
//!   published (or the draft).
//!
//! Each change is kept as activity, audited, and raised as a `kb.*` event.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{KbError, KbResult, render, text};

/// Who may see an article.
pub const VISIBILITIES: &[&str] = &["internal", "clients", "public"];
/// Where an article is in its life.
pub const STATUSES: &[&str] = &["draft", "in_review", "published", "archived"];

/// An article as listed.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbArticleSummary)]
pub struct Summary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub category_id: Option<Uuid>,
    pub category_name: Option<String>,
    pub title: String,
    pub visibility: String,
    pub status: String,
    pub tags: Vec<String>,
    pub revision: i32,
    pub published_revision: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    /// Where the search matched, with the matched words in `<b>`.
    pub snippet: Option<String>,
}

/// What readers see: a published revision.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbPublished)]
pub struct Published {
    pub revision: i32,
    pub title: String,
    pub body: String,
    pub body_html: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
}

/// Something that happened to an article.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbActivity)]
pub struct Activity {
    pub kind: String,
    pub actor_name: Option<String>,
    pub details: Json,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// An article in full. `title`, `body` and `bodyHtml` are the working copy
/// (the latest revision); `published` is what readers see.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbArticle)]
pub struct Article {
    #[serde(flatten)]
    pub summary: Summary,
    pub body: String,
    pub body_html: String,
    pub published: Option<Published>,
    pub created_by_name: Option<String>,
    pub updated_by_name: Option<String>,
    pub review_requested_by_name: Option<String>,
    /// For optimistic locking: send it back when changing the article.
    pub version: i32,
    pub activity: Vec<Activity>,
}

impl Article {
    /// The article as someone who may only read it sees it: the published
    /// revision, without drafts or history. `None` when nothing is
    /// published (or it's archived).
    pub fn for_reader(mut self) -> Option<Self> {
        if self.summary.status == "archived" {
            return None;
        }
        let published = self.published.take()?;
        self.summary.title.clone_from(&published.title);
        self.summary.revision = published.revision;
        self.body.clone_from(&published.body);
        self.body_html.clone_from(&published.body_html);
        self.published = Some(published);
        self.summary.status = "published".into();
        self.activity.clear();
        self.review_requested_by_name = None;
        Some(self)
    }
}

/// A new article.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = KbArticleInput)]
pub struct ArticleInput {
    /// The organization it belongs to: the MSP's for shared know-how, a
    /// client's for its own documentation.
    pub org_id: Uuid,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub category_id: Option<Uuid>,
    /// `internal` (the default), `clients` or `public`.
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Changes to an article: what's given changes.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = KbArticleChanges)]
pub struct ArticleChanges {
    /// The version you read.
    pub version: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// A category, or `null` for none.
    #[serde(default, deserialize_with = "some")]
    #[schema(value_type = Option<Uuid>)]
    pub category_id: Option<Option<Uuid>>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// A present field, even when `null`.
#[allow(clippy::option_option)]
fn some<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Uuid>>, D::Error> {
    Option::<Uuid>::deserialize(d).map(Some)
}

/// Where the caller sees articles as they're being written (drafts,
/// reviews, archived): where they may write. Elsewhere they see only what's
/// published.
#[derive(Debug, Clone, Default)]
pub enum Drafts {
    /// Everywhere in the scope.
    All,
    /// Nowhere: published articles only.
    #[default]
    None,
    /// In these organizations.
    In(Vec<Uuid>),
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Words to search for (title, tags and body, best match first).
    pub q: Option<String>,
    pub category_id: Option<Uuid>,
    pub status: Option<String>,
    /// Only this organization's articles.
    pub org_id: Option<Uuid>,
    /// Where drafts (and the working copy) may be seen.
    pub drafts: Drafts,
    pub limit: i64,
    pub offset: i64,
}

const SUMMARY: &str = "a.id, a.org_id, o.name AS org_name, a.category_id, c.name AS category_name, \
    a.title, a.visibility, a.status, a.tags, a.revision, a.published_revision, a.updated_at, a.published_at";

const FROM: &str = "FROM kb_articles a JOIN organizations o ON o.id = a.org_id \
    LEFT JOIN kb_categories c ON c.id = a.category_id";

/// Articles the transaction's scope can see: by search rank when searching,
/// else most recently changed. With the total. Where the caller may only
/// read (see [`Drafts`]), an article shows as published: its published
/// title, matched and quoted from its published text.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> KbResult<(Vec<Summary>, i64)> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let status = filter.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status
        && !STATUSES.contains(&s)
    {
        return Err(KbError::invalid(
            "status",
            "draft, in_review, published or archived",
        ));
    }
    let (all, orgs) = match &filter.drafts {
        Drafts::All => (true, Vec::new()),
        Drafts::None => (false, Vec::new()),
        Drafts::In(orgs) => (false, orgs.clone()),
    };
    // Whether this row's working copy may be seen.
    let own = "($4 OR a.org_id = ANY($7))";
    let published = "(a.published_revision IS NOT NULL AND a.status <> 'archived')";
    let query = "websearch_to_tsquery('english', $1)";
    let wh = format!(
        "WHERE ({own} OR {published}) \
           AND ($1::text IS NULL \
                OR (CASE WHEN {own} OR a.published_revision = a.revision THEN a.search @@ {query} \
                         ELSE to_tsvector('english', coalesce(r.title, '') || ' ' || coalesce(r.body, '')) @@ {query} END) \
                OR (CASE WHEN {own} THEN a.title ELSE r.title END) ILIKE '%' || $1 || '%') \
           AND ($2::uuid IS NULL OR a.category_id = $2) \
           AND ($3::text IS NULL OR (CASE WHEN {own} THEN a.status ELSE 'published' END) = $3) \
           AND ($8::uuid IS NULL OR a.org_id = $8)"
    );
    let headline = crate::highlight::options(30, 12);
    let columns = format!(
        "a.id, a.org_id, o.name AS org_name, a.category_id, c.name AS category_name, \
         CASE WHEN {own} THEN a.title ELSE r.title END AS title, a.visibility, \
         CASE WHEN {own} THEN a.status ELSE 'published' END AS status, a.tags, \
         CASE WHEN {own} THEN a.revision ELSE a.published_revision END AS revision, \
         a.published_revision, a.updated_at, a.published_at, \
         CASE WHEN $1::text IS NULL THEN NULL ELSE ts_headline('english', \
              CASE WHEN {own} THEN a.body ELSE coalesce(r.body, '') END, {query}, \
              '{headline}') END AS snippet"
    );
    let from = format!(
        "{FROM} LEFT JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision"
    );
    let rows: Vec<Summary> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {columns} {from} {wh} \
         ORDER BY CASE WHEN $1::text IS NULL THEN 0 ELSE ts_rank(a.search, {query}) END DESC, \
                  a.updated_at DESC \
         LIMIT $5 OFFSET $6"
    )))
    .bind(q)
    .bind(filter.category_id)
    .bind(status)
    .bind(all)
    .bind(filter.limit.clamp(1, 200))
    .bind(filter.offset.max(0))
    .bind(&orgs)
    .bind(filter.org_id)
    .fetch_all(&mut *conn)
    .await?;
    let rows: Vec<Summary> = rows
        .into_iter()
        .map(|mut r| {
            r.snippet = r.snippet.as_deref().map(crate::highlight::to_html);
            r
        })
        .collect();
    let total: i64 =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from} {wh}")))
            .bind(q)
            .bind(filter.category_id)
            .bind(status)
            .bind(all)
            // Not used by the count; bound so $7 keeps its place.
            .bind(0_i64)
            .bind(0_i64)
            .bind(&orgs)
            .bind(filter.org_id)
            .fetch_one(conn)
            .await?;
    Ok((rows, total))
}

#[derive(sqlx::FromRow)]
struct Row {
    #[sqlx(flatten)]
    summary: Summary,
    body: String,
    version: i32,
    created_by_name: Option<String>,
    updated_by_name: Option<String>,
    review_requested_by_name: Option<String>,
    published_title: Option<String>,
    published_body: Option<String>,
}

/// An article in full.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> KbResult<Article> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY}, NULL::text AS snippet, a.body, a.version, \
                cu.display_name AS created_by_name, uu.display_name AS updated_by_name, \
                ru.display_name AS review_requested_by_name, \
                r.title AS published_title, r.body AS published_body \
           {FROM} \
           LEFT JOIN users cu ON cu.id = a.created_by \
           LEFT JOIN users uu ON uu.id = a.updated_by \
           LEFT JOIN users ru ON ru.id = a.review_requested_by \
           LEFT JOIN kb_revisions r ON r.article_id = a.id AND r.number = a.published_revision \
          WHERE a.id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(KbError::NotFound)?;
    let activity: Vec<Activity> = sqlx::query_as(
        "SELECT k.kind, u.display_name AS actor_name, k.details, k.created_at \
           FROM kb_activity k LEFT JOIN users u ON u.id = k.actor_id \
          WHERE k.article_id = $1 ORDER BY k.seq",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    let published = match (
        row.summary.published_revision,
        row.published_title,
        row.published_body,
    ) {
        (Some(revision), Some(title), Some(body)) => Some(Published {
            revision,
            body_html: render(&body),
            title,
            body,
            published_at: row.summary.published_at,
        }),
        _ => None,
    };
    Ok(Article {
        body_html: render(&row.body),
        body: row.body,
        summary: row.summary,
        published,
        created_by_name: row.created_by_name,
        updated_by_name: row.updated_by_name,
        review_requested_by_name: row.review_requested_by_name,
        version: row.version,
        activity,
    })
}

fn visibility(value: Option<&str>) -> KbResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(v) if VISIBILITIES.contains(&v) => Ok(Some(v.to_owned())),
        Some(_) => Err(KbError::invalid(
            "visibility",
            "internal, clients or public",
        )),
    }
}

fn tags(value: Option<&[String]>) -> KbResult<Option<Vec<String>>> {
    let Some(value) = value else { return Ok(None) };
    let mut out: Vec<String> = Vec::new();
    for tag in value {
        let Some(tag) = text("tags", Some(tag), 50)? else {
            continue;
        };
        let tag = tag.to_lowercase();
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > 20 {
        return Err(KbError::invalid("tags", "at most 20 tags"));
    }
    Ok(Some(out))
}

/// Checks a category is one the transaction can see, at the article's
/// organization or above it.
async fn category(conn: &mut PgConnection, org_id: Uuid, id: Option<Uuid>) -> KbResult<()> {
    let Some(id) = id else { return Ok(()) };
    let fits: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM kb_categories c JOIN organizations co ON co.id = c.org_id \
           WHERE c.id = $1 AND co.path @> (SELECT path FROM organizations WHERE id = $2))",
    )
    .bind(id)
    .bind(org_id)
    .fetch_one(conn)
    .await?;
    if fits {
        Ok(())
    } else {
        Err(KbError::invalid(
            "categoryId",
            "there's no such category here",
        ))
    }
}

/// Keeps a revision of the working copy.
async fn revise(conn: &mut PgConnection, ctx: &RequestContext, a: &Article) -> KbResult<()> {
    sqlx::query(
        "INSERT INTO kb_revisions (article_id, number, org_id, title, body, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(a.summary.id)
    .bind(a.summary.revision)
    .bind(a.summary.org_id)
    .bind(&a.summary.title)
    .bind(&a.body)
    .bind(ctx.actor.id())
    .execute(conn)
    .await?;
    Ok(())
}

/// Notes, audits and announces a change.
async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    a: &Article,
    kind: &str,
    details: Json,
) -> KbResult<()> {
    sqlx::query(
        "INSERT INTO kb_activity (id, article_id, org_id, actor_id, kind, details) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(huxley_core::new_id())
    .bind(a.summary.id)
    .bind(a.summary.org_id)
    .bind(ctx.actor.id())
    .bind(kind)
    .bind(&details)
    .execute(&mut *conn)
    .await?;
    let event = match kind {
        "created" => "kb.article_created",
        "submitted" => "kb.review_requested",
        "rejected" => "kb.review_rejected",
        "published" => "kb.article_published",
        "archived" => "kb.article_archived",
        _ => "kb.article_updated",
    };
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(a.summary.org_id)
            .target("kb_article", a.summary.id)
            .details(json!({ "title": a.summary.title, "change": details })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(a.summary.org_id),
            subject_type: "kb_article",
            subject_id: a.summary.id,
            payload: json!({
                "title": a.summary.title,
                "status": a.summary.status,
                "visibility": a.summary.visibility,
                "revision": a.summary.revision,
                "actorId": ctx.actor.id(),
                "change": details,
            }),
        },
    )
    .await?;
    Ok(())
}

/// Writes a new article (a draft, revision 1).
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &ArticleInput,
) -> KbResult<Article> {
    let title = text("title", Some(&input.title), 300)?
        .ok_or_else(|| KbError::invalid("title", "an article needs a title"))?;
    let body = text("body", input.body.as_deref(), 200_000)?.unwrap_or_default();
    let visibility = visibility(input.visibility.as_deref())?.unwrap_or_else(|| "internal".into());
    let tags = tags(input.tags.as_deref())?.unwrap_or_default();
    category(conn, input.org_id, input.category_id).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO kb_articles (id, org_id, category_id, title, body, visibility, tags, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)",
    )
    .bind(id)
    .bind(input.org_id)
    .bind(input.category_id)
    .bind(&title)
    .bind(&body)
    .bind(&visibility)
    .bind(&tags)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let article = get(conn, id).await?;
    revise(conn, ctx, &article).await?;
    record(conn, ctx, &article, "created", json!({})).await?;
    get(conn, id).await
}

/// Changes an article. A new title or body is a new revision, and makes a
/// published (or in review) article a draft again; readers keep the
/// published revision. Archived articles can't be changed.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    changes: &ArticleChanges,
) -> KbResult<Article> {
    let before = get(conn, id).await?;
    if before.version != changes.version {
        return Err(KbError::VersionConflict);
    }
    if before.summary.status == "archived" {
        return Err(KbError::State(
            "restore the article before changing it".into(),
        ));
    }
    let title = match &changes.title {
        Some(t) => text("title", Some(t), 300)?
            .ok_or_else(|| KbError::invalid("title", "an article needs a title"))?,
        None => before.summary.title.clone(),
    };
    let body = match &changes.body {
        Some(b) => text("body", Some(b), 200_000)?.unwrap_or_default(),
        None => before.body.clone(),
    };
    let visibility = visibility(changes.visibility.as_deref())?
        .unwrap_or_else(|| before.summary.visibility.clone());
    let tags = tags(changes.tags.as_deref())?.unwrap_or_else(|| before.summary.tags.clone());
    let category_id = changes.category_id.unwrap_or(before.summary.category_id);
    category(conn, before.summary.org_id, category_id).await?;
    let content = title != before.summary.title || body != before.body;
    let mut fields: Vec<&str> = Vec::new();
    if title != before.summary.title {
        fields.push("title");
    }
    if body != before.body {
        fields.push("body");
    }
    if visibility != before.summary.visibility {
        fields.push("visibility");
    }
    if tags != before.summary.tags {
        fields.push("tags");
    }
    if category_id != before.summary.category_id {
        fields.push("category");
    }
    if fields.is_empty() {
        return Ok(before);
    }
    let rows = sqlx::query(
        "UPDATE kb_articles SET title = $3, body = $4, visibility = $5, tags = $6, category_id = $7, \
                revision = revision + CASE WHEN $8 THEN 1 ELSE 0 END, \
                status = CASE WHEN $8 THEN 'draft' ELSE status END, \
                review_requested_by = CASE WHEN $8 THEN NULL ELSE review_requested_by END, \
                updated_by = $9, updated_at = now(), version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(changes.version)
    .bind(&title)
    .bind(&body)
    .bind(&visibility)
    .bind(&tags)
    .bind(category_id)
    .bind(content)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(KbError::VersionConflict);
    }
    let article = get(conn, id).await?;
    if content {
        revise(conn, ctx, &article).await?;
    }
    record(
        conn,
        ctx,
        &article,
        "edited",
        json!({ "fields": fields, "revision": article.summary.revision }),
    )
    .await?;
    get(conn, id).await
}

/// Moves an article from one of `from` to `to`, or says why it can't.
async fn transition(
    conn: &mut PgConnection,
    id: Uuid,
    from: &[&str],
    to: &str,
    set: &str,
    why: &str,
    actor: Option<Uuid>,
) -> KbResult<()> {
    let from: Vec<String> = from.iter().map(|s| (*s).to_owned()).collect();
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE kb_articles SET status = $3, {set} updated_at = now(), version = version + 1 \
          WHERE id = $1 AND status = ANY($2)"
    )))
    .bind(id)
    .bind(&from)
    .bind(to)
    .bind(actor)
    .execute(conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(KbError::State(why.into()));
    }
    Ok(())
}

/// Submits a draft for review.
pub async fn submit(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> KbResult<Article> {
    get(conn, id).await?;
    transition(
        conn,
        id,
        &["draft"],
        "in_review",
        "review_requested_by = $4,",
        "only a draft can be submitted for review",
        ctx.actor.id(),
    )
    .await?;
    let article = get(conn, id).await?;
    record(
        conn,
        ctx,
        &article,
        "submitted",
        json!({ "revision": article.summary.revision }),
    )
    .await?;
    get(conn, id).await
}

/// Publishes the working copy: approving one in review, or publishing a
/// draft directly. Readers see it from now on.
pub async fn publish(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    comment: Option<&str>,
) -> KbResult<Article> {
    let before = get(conn, id).await?;
    let comment = text("comment", comment, 2000)?;
    transition(
        conn,
        id,
        &["draft", "in_review"],
        "published",
        "published_revision = revision, published_at = now(), review_requested_by = NULL,",
        if before.summary.status == "published" {
            "this revision is already published"
        } else {
            "an archived article can't be published; restore it first"
        },
        None,
    )
    .await?;
    let article = get(conn, id).await?;
    record(
        conn,
        ctx,
        &article,
        "published",
        json!({
            "revision": article.summary.revision,
            "reviewed": before.summary.status == "in_review",
            "comment": comment,
        }),
    )
    .await?;
    get(conn, id).await
}

/// Sends an article in review back to its author as a draft, saying why.
pub async fn reject(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    comment: &str,
) -> KbResult<Article> {
    let before = get(conn, id).await?;
    let comment = text("comment", Some(comment), 2000)?
        .ok_or_else(|| KbError::invalid("comment", "say what needs changing"))?;
    transition(
        conn,
        id,
        &["in_review"],
        "draft",
        "",
        "only an article in review can be rejected",
        None,
    )
    .await?;
    let article = get(conn, id).await?;
    record(
        conn,
        ctx,
        &article,
        "rejected",
        json!({
            "revision": article.summary.revision,
            "comment": comment,
            "requestedBy": before.review_requested_by_name,
        }),
    )
    .await?;
    get(conn, id).await
}

/// Hides an article from readers.
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> KbResult<Article> {
    get(conn, id).await?;
    transition(
        conn,
        id,
        &["draft", "in_review", "published"],
        "archived",
        "review_requested_by = NULL,",
        "it's already archived",
        None,
    )
    .await?;
    let article = get(conn, id).await?;
    record(conn, ctx, &article, "archived", json!({})).await?;
    get(conn, id).await
}

/// Brings an archived article back: published again if it had been
/// (and nothing changed since), else as a draft.
pub async fn restore(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> KbResult<Article> {
    let before = get(conn, id).await?;
    let to = if before.summary.published_revision == Some(before.summary.revision) {
        "published"
    } else {
        "draft"
    };
    transition(
        conn,
        id,
        &["archived"],
        to,
        "",
        "only an archived article can be restored",
        None,
    )
    .await?;
    let article = get(conn, id).await?;
    record(conn, ctx, &article, "restored", json!({ "status": to })).await?;
    get(conn, id).await
}

/// A saved revision.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbRevision)]
pub struct Revision {
    pub number: i32,
    pub title: String,
    /// Markdown (left out when listing).
    pub body: Option<String>,
    pub body_html: Option<String>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// An article's revisions, newest first (without their bodies).
pub async fn revisions(conn: &mut PgConnection, id: Uuid) -> KbResult<Vec<Revision>> {
    get(conn, id).await?;
    Ok(sqlx::query_as(
        "SELECT r.number, r.title, NULL::text AS body, NULL::text AS body_html, \
                u.display_name AS created_by_name, r.created_at \
           FROM kb_revisions r LEFT JOIN users u ON u.id = r.created_by \
          WHERE r.article_id = $1 ORDER BY r.number DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// One revision, with its body.
pub async fn revision(conn: &mut PgConnection, id: Uuid, number: i32) -> KbResult<Revision> {
    let mut r: Revision = sqlx::query_as(
        "SELECT r.number, r.title, r.body, NULL::text AS body_html, \
                u.display_name AS created_by_name, r.created_at \
           FROM kb_revisions r LEFT JOIN users u ON u.id = r.created_by \
          WHERE r.article_id = $1 AND r.number = $2",
    )
    .bind(id)
    .bind(number)
    .fetch_optional(conn)
    .await?
    .ok_or(KbError::NotFound)?;
    r.body_html = r.body.as_deref().map(render);
    Ok(r)
}
