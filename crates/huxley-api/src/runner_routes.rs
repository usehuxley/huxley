//! `/api/v1/runners`: script runners (design doc §6.9, ADR-025). Admins
//! enrol, register and revoke them; a runner uses its enrolment token once
//! to get its client certificate for the gateway.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_runners::registry::{self, RegistryError, RunnerView};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/enrollments", post(create_enrollment))
        .route("/enroll", post(enroll))
        .route("/external", post(register_external))
        .route("/{id}/revoke", post(revoke))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: RegistryError) -> ApiError {
    match e {
        RegistryError::NotFound => ApiError::not_found(),
        RegistryError::NameTaken(_) => ApiError::conflict(e.to_string()),
        RegistryError::BadToken => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad-enrollment-token",
            "The enrolment token is unknown, used or expired",
        ),
        RegistryError::Invalid(message) => ApiError::validation().with_detail(message),
        other => ApiError::internal(&other),
    }
}

/// Every runner, active ones first.
#[utoipa::path(get, path = "/api/v1/runners", operation_id = "list_runners", tag = "runners",
    responses((status = 200, body = Vec<RunnerView>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<RunnerView>>> {
    authed.require_instance(permissions::RUNNERS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let runners = registry::list(&mut tx).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(runners))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunnerNameRequest {
    /// Lowercase letters, digits, dots and hyphens.
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentToken {
    /// Shown once: the runner uses it with `huxley runner enroll`.
    pub token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// A single-use token for a new runner to enrol with (valid 24 hours).
#[utoipa::path(post, path = "/api/v1/runners/enrollments", operation_id = "create_runner_enrollment", tag = "runners",
    request_body = RunnerNameRequest,
    responses((status = 201, body = EnrollmentToken), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_enrollment(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<RunnerNameRequest>,
) -> ApiResult<(StatusCode, Json<EnrollmentToken>)> {
    authed.require_instance(permissions::RUNNERS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (token, expires_at) = registry::create_enrollment(&mut tx, &authed.user.ctx, &body.name)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(EnrollmentToken { token, expires_at }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollRequest {
    pub token: String,
    /// PEM certificate signing request for the runner's own key.
    pub csr_pem: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub runner_id: Uuid,
    pub name: String,
    pub certificate_pem: String,
    /// The CA to trust for the gateway.
    pub ca_pem: String,
    /// Where to connect (`runners.public_url`), if configured.
    pub gateway_url: Option<String>,
}

/// A runner enrols: the token (used once) proves who sent it. No session.
#[utoipa::path(post, path = "/api/v1/runners/enroll", operation_id = "enroll_runner", tag = "runners",
    request_body = EnrollRequest,
    responses((status = 200, body = EnrollResponse), (status = 401, body = ApiError, content_type = "application/problem+json")))]
pub async fn enroll(
    State(state): State<AppState>,
    Json(body): Json<EnrollRequest>,
) -> ApiResult<Json<EnrollResponse>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let enrolled = registry::enroll(&mut tx, &state.crypto, &body.token, &body.csr_pem)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(EnrollResponse {
        runner_id: enrolled.runner.id,
        name: enrolled.runner.name,
        certificate_pem: enrolled.certificate_pem,
        ca_pem: enrolled.ca_pem,
        gateway_url: state.runner_gateway_url.clone(),
    }))
}

/// Registers a runner whose certificate comes from your own CA
/// (`runners.client_ca_file`); its certificate's common name is `name`.
#[utoipa::path(post, path = "/api/v1/runners/external", operation_id = "register_external_runner", tag = "runners",
    request_body = RunnerNameRequest,
    responses((status = 201, body = RunnerView), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn register_external(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<RunnerNameRequest>,
) -> ApiResult<(StatusCode, Json<RunnerView>)> {
    authed.require_instance(permissions::RUNNERS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let runner = registry::register_external(&mut tx, &authed.user.ctx, &body.name)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(runner)))
}

/// Stops a runner at once: the gateway refuses it from its next call.
#[utoipa::path(post, path = "/api/v1/runners/{id}/revoke", operation_id = "revoke_runner", tag = "runners",
    params(("id" = Uuid, Path)), responses((status = 200, body = RunnerView)))]
pub async fn revoke(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunnerView>> {
    authed.require_instance(permissions::RUNNERS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let runner = registry::revoke(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(runner))
}
