//! SIEM audit streaming (ADR-087): where the audit log is shipped as it's
//! written. Needs `platform:audit_streams:manage` (instance-wide).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::EngineError;
use huxley_engine::audit_stream::{self, AuditStream, StreamInput};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/platform/audit-streams", get(list).post(create))
        .route("/platform/audit-streams/{id}", put(update).delete(remove))
        .route("/platform/audit-streams/{id}/test", post(test))
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

fn not_found() -> ApiError {
    ApiError::not_found()
}

#[utoipa::path(get, path = "/api/v1/platform/audit-streams", operation_id = "list_audit_streams", tag = "audit",
    responses((status = 200, body = Vec<AuditStream>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<AuditStream>>> {
    authed.require_instance(permissions::AUDIT_STREAM_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let streams = audit_stream::list(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(streams))
}

#[utoipa::path(post, path = "/api/v1/platform/audit-streams", operation_id = "create_audit_stream", tag = "audit",
    request_body = StreamInput, responses((status = 201, body = AuditStream)))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<StreamInput>,
) -> ApiResult<(StatusCode, Json<AuditStream>)> {
    authed.require_instance(permissions::AUDIT_STREAM_MANAGE)?;
    let stream = state
        .engine
        .create_audit_stream(&authed.user.ctx, &body)
        .await
        .map_err(map)?;
    Ok((StatusCode::CREATED, Json(stream)))
}

#[utoipa::path(put, path = "/api/v1/platform/audit-streams/{id}", operation_id = "update_audit_stream", tag = "audit",
    request_body = StreamInput, responses((status = 200, body = AuditStream)))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<StreamInput>,
) -> ApiResult<Json<AuditStream>> {
    authed.require_instance(permissions::AUDIT_STREAM_MANAGE)?;
    state
        .engine
        .update_audit_stream(&authed.user.ctx, id, &body)
        .await
        .map_err(map)?
        .map(Json)
        .ok_or_else(not_found)
}

#[utoipa::path(delete, path = "/api/v1/platform/audit-streams/{id}", operation_id = "delete_audit_stream", tag = "audit",
    responses((status = 204)))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::AUDIT_STREAM_MANAGE)?;
    if state
        .engine
        .delete_audit_stream(&authed.user.ctx, id)
        .await
        .map_err(map)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

/// How a test event went.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamTest {
    pub ok: bool,
    pub error: Option<String>,
}

/// Sends a test event (not part of the audit log) to the stream's
/// destination.
#[utoipa::path(post, path = "/api/v1/platform/audit-streams/{id}/test", operation_id = "test_audit_stream", tag = "audit",
    responses((status = 200, body = StreamTest)))]
pub async fn test(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<StreamTest>> {
    authed.require_instance(permissions::AUDIT_STREAM_MANAGE)?;
    let outcome = state
        .engine
        .test_audit_stream(id)
        .await
        .map_err(map)?
        .ok_or_else(not_found)?;
    Ok(Json(StreamTest {
        ok: outcome.is_ok(),
        error: outcome.err(),
    }))
}
