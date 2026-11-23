//! `/api/v1/orgs/{id}/variables`: organization variables for workflows.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::begin_scoped;
use huxley_variables::{ResolvedVariable, VariableError, VariableInput};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{id}/variables", get(list))
        .route("/orgs/{id}/variables/{key}", put(set).delete(clear))
}

fn map(e: VariableError) -> ApiError {
    match e {
        VariableError::OrgNotFound | VariableError::NotFound => ApiError::not_found(),
        VariableError::InvalidKey | VariableError::InvalidValue(_) => {
            ApiError::validation().with_detail(e.to_string())
        }
        VariableError::Locked(_) => ApiError::new(
            StatusCode::CONFLICT,
            "variable-locked",
            "This variable is locked by an organization above",
        )
        .with_detail(e.to_string()),
        VariableError::Crypto(e) => ApiError::internal(&e),
        VariableError::Db(e) => ApiError::internal(&e),
    }
}

/// The variables in effect at an organization: its own and inherited ones.
/// Secret values are never returned.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/variables", operation_id = "list_org_variables", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<ResolvedVariable>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<ResolvedVariable>>> {
    authed
        .require_at(&state, permissions::WORKFLOW_READ, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let variables = huxley_variables::resolve(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(variables))
}

/// Sets a variable at an organization (for it and those below).
#[utoipa::path(put, path = "/api/v1/orgs/{id}/variables/{key}", operation_id = "set_org_variable", tag = "workflows",
    params(("id" = Uuid, Path), ("key" = String, Path)), request_body = VariableInput,
    responses(
        (status = 204),
        (status = 409, description = "Locked above", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn set(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key)): Path<(Uuid, String)>,
    Json(body): Json<VariableInput>,
) -> ApiResult<StatusCode> {
    authed
        .require_at(&state, permissions::VARIABLE_MANAGE, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    huxley_variables::set(&mut tx, &authed.user.ctx, &state.crypto, id, &key, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a variable from an organization; it inherits again.
#[utoipa::path(delete, path = "/api/v1/orgs/{id}/variables/{key}", operation_id = "clear_org_variable", tag = "workflows",
    params(("id" = Uuid, Path), ("key" = String, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn clear(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key)): Path<(Uuid, String)>,
) -> ApiResult<StatusCode> {
    authed
        .require_at(&state, permissions::VARIABLE_MANAGE, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    huxley_variables::clear(&mut tx, &authed.user.ctx, id, &key)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(StatusCode::NO_CONTENT)
}
