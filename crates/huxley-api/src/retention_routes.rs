//! Data retention (ADR-085): how long finished workflow runs, events,
//! notifications and AI call records are kept; the daily sweep, or one now.
//! Needs `platform:retention:manage` (instance-wide).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::EngineError;
use huxley_engine::retention::{self, Retention, RetentionState, Swept};

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/platform/retention", get(get_retention).put(put_retention))
        .route("/platform/retention/sweep", post(sweep))
}

fn map(e: EngineError) -> ApiError {
    match e {
        EngineError::Invalid(m) => {
            let (field, message) = m.split_once(": ").unwrap_or(("", m.as_str()));
            ApiError::validation().with_field_error(format!("/{field}"), message.to_owned())
        }
        other => ApiError::internal(&other),
    }
}

#[utoipa::path(get, path = "/api/v1/platform/retention", operation_id = "get_retention", tag = "system",
    responses((status = 200, body = RetentionState)))]
pub async fn get_retention(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<RetentionState>> {
    authed.require_instance(permissions::RETENTION_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let r = retention::get(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(r))
}

#[utoipa::path(put, path = "/api/v1/platform/retention", operation_id = "put_retention", tag = "system",
    request_body = Retention, responses((status = 200, body = RetentionState)))]
pub async fn put_retention(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<Retention>,
) -> ApiResult<Json<RetentionState>> {
    authed.require_instance(permissions::RETENTION_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let r = retention::set(&mut tx, &authed.user.ctx, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(r))
}

/// Sweeps now, as the daily sweep does.
#[utoipa::path(post, path = "/api/v1/platform/retention/sweep", operation_id = "sweep_retention", tag = "system",
    responses((status = 200, body = Swept)))]
pub async fn sweep(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<Swept>> {
    authed.require_instance(permissions::RETENTION_MANAGE)?;
    Ok(Json(state.engine.sweep_retention().await.map_err(map)?))
}
