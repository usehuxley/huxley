//! The `knowledge` module's API (design doc §8.5): articles, their review
//! and revisions, and categories.
//!
//! - `kb:read` at an organization: its published articles.
//! - `kb:write`: write articles there, see their drafts and history, and
//!   submit them for review.
//! - `kb:publish`: approve or reject reviews, publish directly, archive and
//!   restore, change who can see a published article, and keep categories.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_knowledge::articles::{
    self, Article, ArticleChanges, ArticleInput, Drafts, Filter, Revision, Summary,
};
use huxley_knowledge::categories::{self, Category, CategoryInput};
use huxley_knowledge::links::{self, LinkedTicket, TicketArticle};
use huxley_knowledge::{KbError, MODULE};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/categories",
            get(list_kb_categories).post(create_kb_category),
        )
        .route(
            "/categories/{id}",
            put(update_kb_category).delete(delete_kb_category),
        )
        .route("/articles", get(list_articles).post(create_article))
        .route("/articles/{id}", get(get_article).patch(update_article))
        .route("/articles/{id}/submit", post(submit_article))
        .route("/articles/{id}/publish", post(publish_article))
        .route("/articles/{id}/reject", post(reject_article))
        .route("/articles/{id}/archive", post(archive_article))
        .route("/articles/{id}/restore", post(restore_article))
        .route("/articles/{id}/revisions", get(list_revisions))
        .route("/articles/{id}/revisions/{number}", get(get_revision))
        .route("/preview", post(preview_article))
        .route("/meaning", get(search_articles_by_meaning))
        .route("/articles/{id}/tickets", get(list_article_tickets))
        .route(
            "/tickets/{ticket_id}/articles",
            get(ticket_articles).post(link_article),
        )
        .route(
            "/tickets/{ticket_id}/articles/{article_id}",
            axum::routing::delete(unlink_article),
        )
}

fn map(e: KbError) -> ApiError {
    match e {
        KbError::NotFound => ApiError::not_found(),
        KbError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        KbError::State(message) => ApiError::conflict(&message),
        KbError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        KbError::Db(e) => ApiError::internal(&e),
        KbError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, KbError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// The organization a record belongs to (404 if there's none).
async fn owner(state: &AppState, table: &str, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT org_id FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

/// Whether the caller holds `permission` at `org_id`.
async fn can_at(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org_id: Uuid,
) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let path = huxley_authz::org_path(&mut tx, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(authed.principal.can_at_path(permission, &path))
}

/// The module is on at `org_id` and the caller holds `permission` there:
/// the scope to work in.
async fn allow(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org_id: Uuid,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org_id).await?;
    authed.require_at(state, permission, org_id).await?;
    authed.scope(state).await
}

/// An article's organization, checked for `permission`.
async fn article_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = owner(state, "kb_articles", id).await?;
    allow(state, authed, permission, org).await
}

// ---- Categories ----

/// Categories you can see, in order.
#[utoipa::path(get, path = "/api/v1/kb/categories", tag = "knowledge",
    responses((status = 200, body = Vec<Category>)))]
pub async fn list_kb_categories(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Category>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::KB_READ, MODULE)
        .await?;
    Ok(Json(scoped(&state, &scope, categories::list).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewKbCategory {
    pub org_id: Uuid,
    pub category: CategoryInput,
}

/// Adds a category at an organization (`kb:publish` there). Articles there
/// and below can use it.
#[utoipa::path(post, path = "/api/v1/kb/categories", tag = "knowledge", request_body = NewKbCategory,
    responses((status = 201, body = Category), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_kb_category(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<NewKbCategory>,
) -> ApiResult<(StatusCode, Json<Category>)> {
    let scope = allow(&state, &authed, permissions::KB_PUBLISH, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let category = scoped(&state, &scope, async |tx| {
        categories::create(tx, &ctx, body.org_id, &body.category).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(category)))
}

#[utoipa::path(put, path = "/api/v1/kb/categories/{id}", tag = "knowledge", params(("id" = Uuid, Path)),
    request_body = CategoryInput,
    responses((status = 200, body = Category), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_kb_category(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CategoryInput>,
) -> ApiResult<Json<Category>> {
    let org = owner(&state, "kb_categories", id).await?;
    let scope = allow(&state, &authed, permissions::KB_PUBLISH, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            categories::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Deletes a category; its articles stay, uncategorized.
#[utoipa::path(delete, path = "/api/v1/kb/categories/{id}", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 204)))]
pub async fn delete_kb_category(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = owner(&state, "kb_categories", id).await?;
    let scope = allow(&state, &authed, permissions::KB_PUBLISH, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        categories::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Articles ----

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ArticleParams {
    /// Words to search for in titles, tags and text.
    pub q: Option<String>,
    pub category_id: Option<Uuid>,
    /// `draft`, `in_review`, `published` or `archived`.
    pub status: Option<String>,
    /// Only this organization's articles (e.g. a client's documentation).
    pub org_id: Option<Uuid>,
    /// Default 50, at most 200.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArticlePage {
    pub items: Vec<Summary>,
    pub total: i64,
}

/// Articles you can see, best match first when searching. Where you may
/// write you see drafts and reviews too; elsewhere, what's published.
#[utoipa::path(get, path = "/api/v1/kb/articles", tag = "knowledge", params(ArticleParams),
    responses((status = 200, body = ArticlePage)))]
pub async fn list_articles(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<ArticleParams>,
) -> ApiResult<Json<ArticlePage>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::KB_READ, MODULE)
        .await?;
    let drafts = drafts_for(&state, &authed).await?;
    let filter = Filter {
        q: params.q,
        category_id: params.category_id,
        status: params.status,
        org_id: params.org_id,
        drafts,
        limit: params.limit.unwrap_or(50),
        offset: params.offset.unwrap_or(0),
    };
    let (items, total) =
        scoped(&state, &scope, async |tx| articles::list(tx, &filter).await).await?;
    Ok(Json(ArticlePage { items, total }))
}

/// Where the caller sees drafts: where they may write.
async fn drafts_for(state: &AppState, authed: &Authed) -> ApiResult<Drafts> {
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::KB_WRITE)
    {
        return Ok(Drafts::None);
    }
    Ok(
        match authed
            .module_scope(state, permissions::KB_WRITE, MODULE)
            .await?
        {
            OrgScope::System => Drafts::All,
            OrgScope::Orgs(orgs) => Drafts::In(orgs),
        },
    )
}

/// An article, with what you may do with it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArticleView {
    #[serde(flatten)]
    pub article: Article,
    /// You may change it and see its drafts and history.
    pub can_write: bool,
    /// You may approve, reject, publish and archive it.
    pub can_publish: bool,
}

async fn view(state: &AppState, authed: &Authed, article: Article) -> ApiResult<ArticleView> {
    let org = article.summary.org_id;
    Ok(ArticleView {
        article,
        can_write: can_at(state, authed, permissions::KB_WRITE, org).await?,
        can_publish: can_at(state, authed, permissions::KB_PUBLISH, org).await?,
    })
}

/// Writes a new article: a draft, at an organization where you hold
/// `kb:write`.
#[utoipa::path(post, path = "/api/v1/kb/articles", tag = "knowledge", request_body = ArticleInput,
    responses((status = 201, body = ArticleView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_article(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ArticleInput>,
) -> ApiResult<(StatusCode, Json<ArticleView>)> {
    let scope = allow(&state, &authed, permissions::KB_WRITE, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let article = scoped(&state, &scope, async |tx| {
        articles::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state, &authed, article).await?),
    ))
}

/// An article. With `kb:write` there, its working copy and history; with
/// only `kb:read`, what's published (404 if nothing is).
#[utoipa::path(get, path = "/api/v1/kb/articles/{id}", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = ArticleView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_READ).await?;
    let article = scoped(&state, &scope, async |tx| articles::get(tx, id).await).await?;
    let view = view(&state, &authed, article).await?;
    if view.can_write {
        return Ok(Json(view));
    }
    let article = view.article.for_reader().ok_or_else(ApiError::not_found)?;
    Ok(Json(ArticleView {
        article,
        can_write: false,
        can_publish: view.can_publish,
    }))
}

/// Changes an article; send back the `version` you read. A new title or
/// body is a new revision and a draft (readers keep the published one).
/// Changing who can see a published article needs `kb:publish`.
#[utoipa::path(patch, path = "/api/v1/kb/articles/{id}", tag = "knowledge", params(("id" = Uuid, Path)),
    request_body = ArticleChanges,
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ArticleChanges>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let before = scoped(&state, &scope, async |tx| articles::get(tx, id).await).await?;
    let exposes = body
        .visibility
        .as_deref()
        .is_some_and(|v| v != before.summary.visibility);
    if exposes
        && before.summary.published_revision.is_some()
        && !can_at(
            &state,
            &authed,
            permissions::KB_PUBLISH,
            before.summary.org_id,
        )
        .await?
    {
        return Err(ApiError::forbidden().with_detail(
            "Changing who can see a published article needs permission to publish (kb:publish).",
        ));
    }
    let article = scoped(&state, &scope, async |tx| {
        articles::update(tx, &ctx, id, &body).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewDecision {
    /// Required to reject: what needs changing.
    #[serde(default)]
    pub comment: Option<String>,
}

/// Submits a draft for review (`kb:write`).
#[utoipa::path(post, path = "/api/v1/kb/articles/{id}/submit", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let article = scoped(&state, &scope, async |tx| {
        articles::submit(tx, &ctx, id).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

/// Publishes the working copy (`kb:publish`): approves one in review, or
/// publishes a draft directly. An optional comment is kept.
#[utoipa::path(post, path = "/api/v1/kb/articles/{id}/publish", tag = "knowledge", params(("id" = Uuid, Path)),
    request_body = ReviewDecision,
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn publish_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    body: Option<Json<ReviewDecision>>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_PUBLISH).await?;
    let ctx = authed.user.ctx.clone();
    let comment = body.and_then(|Json(b)| b.comment);
    let article = scoped(&state, &scope, async |tx| {
        articles::publish(tx, &ctx, id, comment.as_deref()).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

/// Sends an article in review back as a draft, saying what needs changing
/// (`kb:publish`).
#[utoipa::path(post, path = "/api/v1/kb/articles/{id}/reject", tag = "knowledge", params(("id" = Uuid, Path)),
    request_body = ReviewDecision,
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn reject_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ReviewDecision>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_PUBLISH).await?;
    let ctx = authed.user.ctx.clone();
    let comment = body.comment.unwrap_or_default();
    let article = scoped(&state, &scope, async |tx| {
        articles::reject(tx, &ctx, id, &comment).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

/// Hides an article from readers (`kb:publish`).
#[utoipa::path(post, path = "/api/v1/kb/articles/{id}/archive", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_PUBLISH).await?;
    let ctx = authed.user.ctx.clone();
    let article = scoped(&state, &scope, async |tx| {
        articles::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

/// Brings back an archived article: published again if nothing changed
/// since, else as a draft (`kb:publish`).
#[utoipa::path(post, path = "/api/v1/kb/articles/{id}/restore", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = ArticleView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn restore_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ArticleView>> {
    let scope = article_access(&state, &authed, id, permissions::KB_PUBLISH).await?;
    let ctx = authed.user.ctx.clone();
    let article = scoped(&state, &scope, async |tx| {
        articles::restore(tx, &ctx, id).await
    })
    .await?;
    Ok(Json(view(&state, &authed, article).await?))
}

/// An article's revisions, newest first (`kb:write`).
#[utoipa::path(get, path = "/api/v1/kb/articles/{id}/revisions", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<Revision>)))]
pub async fn list_revisions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Revision>>> {
    let scope = article_access(&state, &authed, id, permissions::KB_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| articles::revisions(tx, id).await).await?,
    ))
}

/// One revision, with its text (`kb:write`).
#[utoipa::path(get, path = "/api/v1/kb/articles/{id}/revisions/{number}", tag = "knowledge",
    params(("id" = Uuid, Path), ("number" = i32, Path)),
    responses((status = 200, body = Revision), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_revision(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, number)): Path<(Uuid, i32)>,
) -> ApiResult<Json<Revision>> {
    let scope = article_access(&state, &authed, id, permissions::KB_WRITE).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            articles::revision(tx, id, number).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = KbPreviewRequest)]
pub struct PreviewRequest {
    /// Markdown.
    pub body: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = KbPreviewResponse)]
pub struct PreviewResponse {
    /// The markdown as sanitized HTML, as readers will see it.
    pub html: String,
}

/// Renders markdown as an article would show it (for the editor's preview).
#[utoipa::path(post, path = "/api/v1/kb/preview", tag = "knowledge", request_body = PreviewRequest,
    responses((status = 200, body = PreviewResponse), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn preview_article(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<PreviewResponse>> {
    require_module(&state, MODULE).await?;
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::KB_WRITE)
    {
        return Err(ApiError::forbidden());
    }
    if body.body.len() > 200_000 {
        return Err(ApiError::validation().with_field_error("/body", "at most 200000 characters"));
    }
    Ok(Json(PreviewResponse {
        html: huxley_knowledge::render(&body.body),
    }))
}

// ---- Tickets ----

/// A ticket's organization, checked: the knowledge base and the service
/// desk are on there, and the caller holds `permission` on the ticket.
async fn ticket_org(
    state: &AppState,
    authed: &Authed,
    ticket: Uuid,
    permission: &str,
) -> ApiResult<Uuid> {
    require_module(state, MODULE).await?;
    let org = owner(state, "sd_tickets", ticket).await?;
    require_module_at(state, huxley_servicedesk::MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    Ok(org)
}

/// Articles suggested on a ticket.
const SUGGESTIONS: i64 = 5;
/// How close in meaning (cosine similarity) an article must be to a ticket
/// to be suggested by meaning.
const MIN_MEANING: f64 = 0.3;

/// What the knowledge base has for a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketKnowledge {
    /// Articles linked to it.
    pub linked: Vec<TicketArticle>,
    /// Published articles about the same thing: by meaning when tickets and
    /// articles are embedded (with a `score`), then by its words.
    pub suggested: Vec<TicketArticle>,
    /// You may link and unlink articles (`ticket:write` on the ticket).
    pub can_link: bool,
}

/// The articles linked to a ticket, and published articles suggested for
/// it by its title and description (`ticket:read` on the ticket; what you
/// may read of the knowledge base).
#[utoipa::path(get, path = "/api/v1/kb/tickets/{ticket_id}/articles", tag = "knowledge",
    params(("ticket_id" = Uuid, Path)),
    responses((status = 200, body = TicketKnowledge), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn ticket_articles(
    State(state): State<AppState>,
    authed: Authed,
    Path(ticket_id): Path<Uuid>,
) -> ApiResult<Json<TicketKnowledge>> {
    let org = ticket_org(&state, &authed, ticket_id, permissions::TICKET_READ).await?;
    let can_link = can_at(&state, &authed, permissions::TICKET_WRITE, org).await?;
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::KB_READ)
    {
        return Ok(Json(TicketKnowledge {
            linked: Vec::new(),
            suggested: Vec::new(),
            can_link: false,
        }));
    }
    let text: String = {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(db_err)?;
        let text = sqlx::query_scalar(
            "SELECT title || E'\\n' || description FROM sd_tickets WHERE id = $1",
        )
        .bind(ticket_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        text
    };
    let scope = authed
        .module_scope(&state, permissions::KB_READ, MODULE)
        .await?;
    let drafts = drafts_for(&state, &authed).await?;
    let (linked, suggested) = scoped(&state, &scope, async |tx| {
        let linked = links::for_ticket(tx, ticket_id, &drafts).await?;
        let by_words = links::suggest(tx, ticket_id, &text, SUGGESTIONS).await?;
        // By meaning first (the ticket's own embedding, no AI call), then by words.
        let by_meaning = if huxley_knowledge::embeddings::available(tx).await? {
            huxley_knowledge::embeddings::near_ticket(tx, ticket_id, SUGGESTIONS).await?
        } else {
            Vec::new()
        };
        let mut suggested: Vec<TicketArticle> = by_meaning
            .into_iter()
            .filter(|h| h.score >= MIN_MEANING)
            .map(|h| TicketArticle {
                id: h.id,
                title: h.title,
                status: "published".into(),
                org_name: h.org_name,
                category_name: h.category_name,
                linked_by_name: None,
                linked_at: None,
                snippet: None,
                score: Some(h.score),
            })
            .collect();
        for article in by_words {
            if !suggested.iter().any(|s| s.id == article.id) {
                suggested.push(article);
            }
        }
        suggested.truncate(usize::try_from(SUGGESTIONS).unwrap_or(5));
        Ok((linked, suggested))
    })
    .await?;
    Ok(Json(TicketKnowledge {
        linked,
        suggested,
        can_link,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkArticle {
    pub article_id: Uuid,
}

/// Links an article to a ticket (`ticket:write` on the ticket, `kb:read`
/// where the article is). Linking it again is fine.
#[utoipa::path(post, path = "/api/v1/kb/tickets/{ticket_id}/articles", tag = "knowledge",
    params(("ticket_id" = Uuid, Path)), request_body = LinkArticle,
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json"),
              (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn link_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(ticket_id): Path<Uuid>,
    Json(body): Json<LinkArticle>,
) -> ApiResult<StatusCode> {
    ticket_org(&state, &authed, ticket_id, permissions::TICKET_WRITE).await?;
    let article_org = owner(&state, "kb_articles", body.article_id).await?;
    let scope = allow(&state, &authed, permissions::KB_READ, article_org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        links::link(tx, &ctx, body.article_id, ticket_id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes an article's link to a ticket (`ticket:write` on the ticket).
#[utoipa::path(delete, path = "/api/v1/kb/tickets/{ticket_id}/articles/{article_id}", tag = "knowledge",
    params(("ticket_id" = Uuid, Path), ("article_id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn unlink_article(
    State(state): State<AppState>,
    authed: Authed,
    Path((ticket_id, article_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    ticket_org(&state, &authed, ticket_id, permissions::TICKET_WRITE).await?;
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        links::unlink(tx, &ctx, article_id, ticket_id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The tickets an article is linked to that you can see (`kb:write` where
/// the article is). Empty when the service desk is off.
#[utoipa::path(get, path = "/api/v1/kb/articles/{id}/tickets", tag = "knowledge", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<LinkedTicket>)))]
pub async fn list_article_tickets(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<LinkedTicket>>> {
    article_access(&state, &authed, id, permissions::KB_WRITE).await?;
    let on = {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(db_err)?;
        let on = huxley_modules::is_active(&mut tx, huxley_servicedesk::MODULE)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        on
    };
    if !on
        || !authed
            .principal
            .granted_anywhere()
            .contains(&permissions::TICKET_READ)
    {
        return Ok(Json(Vec::new()));
    }
    let scope = authed
        .module_scope(&state, permissions::TICKET_READ, huxley_servicedesk::MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            links::tickets_for_article(tx, id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct KbMeaningParams {
    /// What to look for, in your own words: 2 to 500 characters.
    pub q: String,
    /// The organization whose embeddings provider reads the query (you need
    /// `ai:use` there).
    pub org_id: Uuid,
}

/// Articles found by meaning, or why searching by meaning isn't possible.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KbMeaningSearch {
    /// Whether searching by meaning is set up (pgvector and an embeddings
    /// provider).
    pub available: bool,
    /// Why there are no results, when something stopped the search.
    pub message: Option<String>,
    pub hits: Vec<huxley_knowledge::embeddings::MeaningHit>,
}

impl KbMeaningSearch {
    fn unavailable(message: &str) -> Self {
        Self {
            available: false,
            message: Some(message.to_owned()),
            hits: Vec::new(),
        }
    }
}

/// Searches published articles by meaning: your words are embedded by the
/// organization's embeddings provider (a logged AI call), and the published
/// articles you can read nearest in meaning come back, best first.
#[utoipa::path(get, path = "/api/v1/kb/meaning", tag = "knowledge", params(KbMeaningParams),
    responses((status = 200, body = KbMeaningSearch), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn search_articles_by_meaning(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<KbMeaningParams>,
) -> ApiResult<Json<KbMeaningSearch>> {
    let q = params.q.trim().to_owned();
    if q.chars().count() < 2 || q.chars().count() > 500 {
        return Err(ApiError::validation().with_field_error("/q", "2 to 500 characters"));
    }
    require_module(&state, MODULE).await?;
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::KB_READ)
    {
        return Err(ApiError::forbidden());
    }
    authed
        .require_at(&state, permissions::AI_USE, params.org_id)
        .await?;
    let (available, provider) = {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(db_err)?;
        let available = huxley_knowledge::embeddings::available(&mut tx)
            .await
            .map_err(map)?;
        let provider =
            huxley_ai::store::effective(&mut tx, params.org_id, huxley_ai::store::Use::Embeddings)
                .await
                .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        (available, provider)
    };
    if !available {
        return Ok(Json(KbMeaningSearch::unavailable(
            "Searching by meaning needs the pgvector extension in the database.",
        )));
    }
    let Some(provider) = provider else {
        return Ok(Json(KbMeaningSearch::unavailable(
            "Set up an embeddings provider first (Settings → AI).",
        )));
    };
    let purpose = huxley_ai::Purpose {
        feature: "kb_search",
        run_id: None,
    };
    let vector = match state
        .engine
        .ai()
        .embed(
            params.org_id,
            purpose,
            std::slice::from_ref(&q),
            huxley_ai::embed::InputType::Query,
        )
        .await
    {
        Ok(mut done) if !done.vectors.is_empty() => done.vectors.swap_remove(0),
        Ok(_) => return Err(ApiError::internal(&"the provider returned no embedding")),
        Err(e @ huxley_ai::AiError::Internal(_)) => return Err(ApiError::internal(&e)),
        Err(e) => {
            return Ok(Json(KbMeaningSearch {
                available: true,
                message: Some(e.to_string()),
                hits: Vec::new(),
            }));
        }
    };
    let scope = authed
        .module_scope(&state, permissions::KB_READ, MODULE)
        .await?;
    let hits = scoped(&state, &scope, async |tx| {
        huxley_knowledge::embeddings::search(tx, &provider.model, &vector, 20).await
    })
    .await?;
    Ok(Json(KbMeaningSearch {
        available: true,
        message: None,
        hits,
    }))
}
