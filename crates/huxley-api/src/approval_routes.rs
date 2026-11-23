//! `/api/v1/approvals`: decisions workflows are waiting for. Being one of an
//! approval's approvers is what lets someone see and decide it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::{self, ApprovalView, DecisionError};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{id}", get(detail))
        .route("/{id}/decision", post(decide))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Approvals waiting for you, then the ones you decided in the last 30 days.
#[utoipa::path(get, path = "/api/v1/approvals", operation_id = "list_my_approvals", tag = "workflows",
    responses((status = 200, body = Vec<ApprovalView>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<ApprovalView>>> {
    // Approvers are named by the workflow, wherever its run is.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let approvals = approval::for_approver(&mut tx, authed.user.auth.user.id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(approvals))
}

/// One approval: for its approvers, and anyone who can read the run.
#[utoipa::path(get, path = "/api/v1/approvals/{id}", operation_id = "get_approval", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = ApprovalView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApprovalView>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = approval::approval(&mut tx, id).await.map_err(db_err)?;
    let approver = approval::is_approver(&mut tx, id, authed.user.auth.user.id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let view = found.ok_or_else(ApiError::not_found)?;
    if !approver {
        authed
            .require_at(&state, permissions::WORKFLOW_READ, view.org_id)
            .await?;
    }
    Ok(Json(view))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approve,
    Reject,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub decision: Decision,
    /// Shown on the run, at most 2000 characters.
    pub comment: Option<String>,
}

/// Approves or rejects. The first decision counts; the workflow continues
/// with it as the step's output.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/decision", operation_id = "decide_approval", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = DecisionRequest,
    responses((status = 200, body = ApprovalView),
              (status = 403, body = ApiError, content_type = "application/problem+json"),
              (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn decide(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DecisionRequest>,
) -> ApiResult<Json<ApprovalView>> {
    if body
        .comment
        .as_deref()
        .is_some_and(|c| c.chars().count() > 2000)
    {
        return Err(ApiError::validation().with_field_error("comment", "at most 2000 characters"));
    }
    let view = state
        .engine
        .decide_approval(
            &authed.user.ctx,
            id,
            authed.user.auth.user.id,
            body.decision == Decision::Approve,
            body.comment.as_deref(),
        )
        .await
        .map_err(|e| match e {
            DecisionError::NotFound => ApiError::not_found(),
            DecisionError::NotAnApprover => ApiError::new(
                StatusCode::FORBIDDEN,
                "not-an-approver",
                "Only its approvers can decide",
            ),
            DecisionError::Closed => ApiError::conflict(e.to_string()),
            DecisionError::Engine(e) => ApiError::internal(&e),
        })?;
    Ok(Json(view))
}
