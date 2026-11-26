//! The semantic data layer's API (ADR-041): the sources you can query, and
//! queries over them. Each source is read with its own permission, in the
//! organizations where you hold it and its module is on.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_analytics::{Answer, QuerySpec, Scope, Source};
use huxley_db::OrgScope;
use serde::Serialize;
use utoipa::ToSchema;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/analytics/sources", get(list_sources))
        .route("/analytics/query", post(query))
}

/// The organizations where the caller can read `source`.
pub(crate) async fn scope_for(
    state: &AppState,
    authed: &Authed,
    source: &Source,
) -> ApiResult<Scope> {
    match authed
        .permission_scope(state, source.permission, source.module)
        .await?
    {
        OrgScope::System => Ok(Scope::All),
        OrgScope::Orgs(orgs) if orgs.is_empty() => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You can't see this data",
        )),
        OrgScope::Orgs(orgs) => Ok(Scope::Orgs(orgs)),
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsCatalog {
    pub sources: Vec<Source>,
    /// What `within` filters accept, e.g. `last_30_days`.
    pub ranges: Vec<&'static str>,
}

/// The data sources you can query, with their fields.
#[utoipa::path(get, path = "/api/v1/analytics/sources", operation_id = "list_analytics_sources", tag = "analytics",
    responses((status = 200, body = AnalyticsCatalog)))]
pub async fn list_sources(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<AnalyticsCatalog>> {
    let mut sources = Vec::new();
    for source in huxley_analytics::SOURCES {
        if scope_for(&state, &authed, source).await.is_ok() {
            sources.push(source.clone());
        }
    }
    Ok(Json(AnalyticsCatalog {
        sources,
        ranges: huxley_analytics::query::ranges().collect(),
    }))
}

pub(crate) fn map(e: huxley_analytics::AnalyticsError) -> ApiError {
    match e {
        huxley_analytics::AnalyticsError::Invalid(at, message) => {
            ApiError::validation().with_field_error(format!("/{at}"), message)
        }
        huxley_analytics::AnalyticsError::TooSlow => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "query-too-slow",
            "That took too long",
        )
        .with_detail("Narrow it down: a shorter time range, fewer groupings, or a filter."),
        huxley_analytics::AnalyticsError::Db(e) => ApiError::internal(&e),
    }
}

/// Runs a query: measures by dimensions over a source, filtered. Numbers
/// cover only the organizations where you can read the source. Results are
/// reused for up to a minute.
#[utoipa::path(post, path = "/api/v1/analytics/query", operation_id = "run_analytics_query", tag = "analytics",
    request_body = QuerySpec,
    responses(
        (status = 200, body = Answer),
        (status = 403, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn query(
    State(state): State<AppState>,
    authed: Authed,
    Json(spec): Json<QuerySpec>,
) -> ApiResult<Json<Answer>> {
    let source = huxley_analytics::catalog::source(&spec.source).ok_or_else(|| {
        ApiError::validation().with_field_error("/source", "there's no such source")
    })?;
    let scope = scope_for(&state, &authed, source).await?;
    let pool = state.read_db.as_ref().unwrap_or(&state.db);
    let answer = huxley_analytics::run(pool, &state.analytics, &spec, &scope)
        .await
        .map_err(map)?;
    Ok(Json(answer))
}
