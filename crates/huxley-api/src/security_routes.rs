//! `/api/v1/security`: sign-in security policy.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::mfa_policy::{self, MfaPolicy};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::{Authed, map_authz};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mfa-policy", get(get_policy))
        .route("/mfa-policy/roles/{id}", put(set_role))
        .route("/mfa-policy/orgs/{id}", put(set_org))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetRequirementRequest {
    pub required: bool,
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Who must have a second factor.
#[utoipa::path(get, path = "/api/v1/security/mfa-policy", tag = "security",
    responses((status = 200, body = MfaPolicy), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_policy(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<MfaPolicy>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let policy = mfa_policy::get(&mut tx).await.map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(policy))
}

/// Requires (or stops requiring) a second factor for holders of a role.
/// Holders without one can only set one up until they do.
#[utoipa::path(put, path = "/api/v1/security/mfa-policy/roles/{id}", tag = "security",
    params(("id" = Uuid, Path, description = "Role id")),
    request_body = SetRequirementRequest,
    responses((status = 204), (status = 403, body = ApiError, content_type = "application/problem+json"),
        (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_role(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRequirementRequest>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    mfa_policy::set_role(&mut tx, &authed.user.ctx, id, body.required)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Requires (or stops requiring) a second factor for users who can reach an
/// organization. On the root organization, that's every user.
#[utoipa::path(put, path = "/api/v1/security/mfa-policy/orgs/{id}", tag = "security",
    params(("id" = Uuid, Path, description = "Organization id")),
    request_body = SetRequirementRequest,
    responses((status = 204), (status = 403, body = ApiError, content_type = "application/problem+json"),
        (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_org(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRequirementRequest>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    mfa_policy::set_org(&mut tx, &authed.user.ctx, id, body.required)
        .await
        .map_err(map_authz)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
