//! Approvals in the client portal (ADR-040): workflow approvals that name
//! a contact (`contact:<id>`) or the company's client approvers
//! (`client_approvers`). Those contacts see and decide them here.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::approval::{self, DecisionError, PortalApproval, Verdict};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/approvals", get(list_approvals))
        .route("/approvals/{id}", get(get_approval))
        .route("/approvals/{id}/decision", post(decide))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Approvals sent to you: waiting ones first.
#[utoipa::path(get, path = "/api/portal/v1/approvals", operation_id = "portal_approvals", tag = "portal",
    responses((status = 200, body = Vec<PortalApproval>)))]
pub async fn list_approvals(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<PortalApproval>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = approval::portal_approvals(&mut tx, authed.principal.account_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

#[utoipa::path(get, path = "/api/portal/v1/approvals/{id}", operation_id = "portal_approval", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, body = PortalApproval)))]
pub async fn get_approval(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalApproval>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let one = approval::portal_approval(&mut tx, authed.principal.account_id, id)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(one))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortalDecision {
    pub approve: bool,
    /// Ask for changes instead (where the approval allows it).
    #[serde(default)]
    pub request_changes: bool,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Approves, rejects or (where allowed) asks for changes. The first
/// decision counts.
#[utoipa::path(post, path = "/api/portal/v1/approvals/{id}/decision", operation_id = "portal_decide_approval",
    tag = "portal", params(("id" = Uuid, Path)), request_body = PortalDecision,
    responses(
        (status = 200, body = PortalApproval),
        (status = 409, description = "Already decided, or expired", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn decide(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
    Json(body): Json<PortalDecision>,
) -> ApiResult<Json<PortalApproval>> {
    let account = authed.principal.account_id;
    if let Some(c) = &body.comment
        && c.chars().count() > 2000
    {
        return Err(ApiError::validation().with_field_error("/comment", "at most 2000 characters"));
    }
    state
        .engine
        .decide_portal_approval(
            &authed.ctx,
            id,
            account,
            if body.request_changes {
                Verdict::RequestChanges
            } else if body.approve {
                Verdict::Approve
            } else {
                Verdict::Reject
            },
            body.comment.as_deref(),
        )
        .await
        .map_err(|e| match e {
            DecisionError::NotFound | DecisionError::NotAnApprover => ApiError::not_found(),
            DecisionError::Closed => ApiError::conflict("It was already decided, or it expired."),
            DecisionError::ChangesNotAllowed => ApiError::validation().with_field_error(
                "/requestChanges",
                "this approval can only be approved or rejected",
            ),
            DecisionError::Engine(e) => ApiError::internal(&e),
        })?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let one = approval::portal_approval(&mut tx, account, id)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(one))
}
