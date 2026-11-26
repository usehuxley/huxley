//! The client portal's projects (ADR-056): the company's projects marked
//! for the portal, with their milestones, progress and client-facing
//! tasks. Client admins and approvers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use huxley_servicedesk::projects::{self, PortalProject};

use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/projects", get(list))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Your company's projects: milestones, progress and client-facing tasks.
#[utoipa::path(get, path = "/api/portal/v1/projects", operation_id = "portal_projects", tag = "portal",
    responses((status = 200, body = Vec<PortalProject>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<PortalProject>>> {
    if !authed.principal.can(Perm::Projects) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Projects are for your organization's approvers and admins",
        ));
    }
    let org = authed.principal.org_id;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    if !huxley_modules::is_active_at(&mut tx, crate::project_routes::MODULE, org)
        .await
        .map_err(db_err)?
    {
        return Ok(Json(Vec::new()));
    }
    let all = projects::for_portal(&mut tx, org)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}
