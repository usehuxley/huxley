//! Problem management (ADR-055): a problem ticket's root cause,
//! workaround and known-error status, and drafting the workaround into the
//! knowledge base. Incidents link to problems through the ticket links
//! (`problem` / `incident`). Reading needs `ticket:read`, changing
//! `ticket:write`; drafting an article `kb:write` too.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::DeskError;
use huxley_servicedesk::problems::{self, Input, Problem};

/// The change and problem management module.
const MODULE: &str = "change";
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tickets/{id}/problem", get(get_problem).put(save_problem))
        .route("/tickets/{id}/problem/article", post(draft_article))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: DeskError) -> ApiError {
    match e {
        DeskError::NotFound => ApiError::not_found(),
        DeskError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        DeskError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        DeskError::Db(e) => ApiError::internal(&e),
        DeskError::Sqlx(e) => ApiError::internal(&e),
    }
}

async fn allowed(
    state: &AppState,
    authed: &Authed,
    ticket: Uuid,
    permission: &str,
) -> ApiResult<Uuid> {
    let org = crate::recommend_routes::ticket_org(state, ticket).await?;
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    Ok(org)
}

/// A problem ticket's root cause, workaround and known-error status.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/problem", operation_id = "get_problem", tag = "changes",
    params(("id" = Uuid, Path)), responses((status = 200, body = Problem)))]
pub async fn get_problem(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Problem>> {
    allowed(&state, &authed, id, permissions::TICKET_READ).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = problems::get(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(found))
}

/// Changes a problem. A known error needs a root cause and a workaround,
/// and raises `problem.known_error`.
#[utoipa::path(put, path = "/api/v1/tickets/{id}/problem", operation_id = "save_problem", tag = "changes",
    params(("id" = Uuid, Path)), request_body = Input,
    responses((status = 200, body = Problem), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn save_problem(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Input>,
) -> ApiResult<Json<Problem>> {
    allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let saved = problems::save(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(saved))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftedArticle {
    pub article_id: Uuid,
}

/// Drafts a knowledge article from the problem's root cause and
/// workaround, submits it for review and links it to the problem.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/problem/article", operation_id = "draft_problem_article", tag = "changes",
    params(("id" = Uuid, Path)),
    responses((status = 201, body = DraftedArticle), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn draft_article(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<(StatusCode, Json<DraftedArticle>)> {
    let org = allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    require_module_at(&state, huxley_knowledge::MODULE, org).await?;
    authed
        .require_at(&state, permissions::KB_WRITE, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let problem = problems::get(&mut tx, id).await.map_err(map)?;
    if problem.workaround.is_empty() {
        return Err(
            ApiError::validation().with_field_error("/workaround", "write the workaround first")
        );
    }
    let title: String = sqlx::query_scalar("SELECT title FROM sd_tickets WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
    let body = format!(
        "## Symptoms\n\n{title}\n\n## Cause\n\n{}\n\n## Workaround\n\n{}\n",
        if problem.root_cause.is_empty() {
            "Not known yet."
        } else {
            &problem.root_cause
        },
        problem.workaround
    );
    let kb = |e: huxley_knowledge::KbError| ApiError::validation().with_detail(e.to_string());
    let article = huxley_knowledge::articles::create(
        &mut tx,
        &authed.user.ctx,
        &huxley_knowledge::articles::ArticleInput {
            org_id: org,
            title: format!("Known error: {title}").chars().take(300).collect(),
            body: Some(body),
            category_id: None,
            visibility: None,
            tags: Some(vec!["known-error".into()]),
        },
    )
    .await
    .map_err(kb)?;
    let article_id = article.summary.id;
    huxley_knowledge::articles::submit(&mut tx, &authed.user.ctx, article_id)
        .await
        .map_err(kb)?;
    huxley_knowledge::links::link(&mut tx, &authed.user.ctx, article_id, id)
        .await
        .map_err(kb)?;
    problems::set_article(&mut tx, id, article_id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(DraftedArticle { article_id })))
}
