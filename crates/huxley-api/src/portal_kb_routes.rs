//! Help articles in the client portal (ADR-040): published articles meant
//! for clients, from the contact's company and the organizations above it.
//! Needs the `knowledge` module on for the company.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_knowledge::portal::{self, Article, Category, Summary};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/articles", get(list_articles))
        .route("/articles/{id}", get(get_article))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: huxley_knowledge::KbError) -> ApiError {
    match e {
        huxley_knowledge::KbError::NotFound => ApiError::not_found(),
        other => ApiError::internal(&other),
    }
}

async fn require_knowledge(state: &AppState, org: Uuid) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let on = huxley_modules::is_active_at(&mut tx, huxley_knowledge::MODULE, org)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    if on {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "module-disabled",
            "Help articles are turned off",
        ))
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ArticleParams {
    /// Words to search for.
    pub q: Option<String>,
    pub category: Option<Uuid>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PortalArticles {
    pub articles: Vec<Summary>,
    pub categories: Vec<Category>,
}

/// Help articles: searched, or newest first; with their categories.
#[utoipa::path(get, path = "/api/portal/v1/articles", operation_id = "portal_articles", tag = "portal",
    params(ArticleParams), responses((status = 200, body = PortalArticles)))]
pub async fn list_articles(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Query(params): Query<ArticleParams>,
) -> ApiResult<Json<PortalArticles>> {
    let org = authed.principal.org_id;
    require_knowledge(&state, org).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let articles = portal::list(&mut tx, org, params.q.as_deref(), params.category)
        .await
        .map_err(map)?;
    let categories = portal::categories(&mut tx, org).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(PortalArticles {
        articles,
        categories,
    }))
}

#[utoipa::path(get, path = "/api/portal/v1/articles/{id}", operation_id = "portal_article", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, body = Article)))]
pub async fn get_article(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Article>> {
    let org = authed.principal.org_id;
    require_knowledge(&state, org).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let article = portal::get(&mut tx, org, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(article))
}
