//! Suggested time entries (ADR-051): yours, waiting to be accepted (a real
//! entry, needing `ticket:write` where the ticket is) or dismissed.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::suggestions::{self, Accept, Suggestion};
use huxley_servicedesk::time::TimeEntry;
use huxley_servicedesk::{DeskError, MODULE};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/time-suggestions", get(mine))
        .route("/time-suggestions/{id}/accept", post(accept))
        .route("/time-suggestions/{id}/dismiss", post(dismiss))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: DeskError) -> ApiError {
    match e {
        DeskError::NotFound => ApiError::not_found(),
        DeskError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        DeskError::VersionConflict => ApiError::conflict("It changed meanwhile; try again."),
        DeskError::Db(e) => ApiError::internal(&e),
        DeskError::Sqlx(e) => ApiError::internal(&e),
    }
}

/// Time suggested for you (by the AI auto-documentation pack), newest first.
#[utoipa::path(get, path = "/api/v1/time-suggestions", operation_id = "my_time_suggestions", tag = "servicedesk",
    responses((status = 200, body = Vec<Suggestion>)))]
pub async fn mine(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Suggestion>>> {
    require_module(&state, MODULE).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = suggestions::mine(&mut tx, authed.user.auth.user.id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// Accepts a suggestion as a time entry (change the minutes or notes first
/// if you like).
#[utoipa::path(post, path = "/api/v1/time-suggestions/{id}/accept", operation_id = "accept_time_suggestion", tag = "servicedesk",
    params(("id" = Uuid, Path)), request_body = Accept,
    responses((status = 201, body = TimeEntry), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn accept(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Accept>,
) -> ApiResult<(StatusCode, Json<TimeEntry>)> {
    require_module(&state, MODULE).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> =
        sqlx::query_scalar("SELECT org_id FROM sd_time_suggestions WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(authed.user.auth.user.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let org = org.ok_or_else(ApiError::not_found)?;
    authed
        .require_at(&state, permissions::TICKET_WRITE, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let entry = suggestions::accept(&mut tx, authed.user.auth.user.id, id, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(entry)))
}

#[utoipa::path(post, path = "/api/v1/time-suggestions/{id}/dismiss", operation_id = "dismiss_time_suggestion", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn dismiss(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_module(&state, MODULE).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let done = suggestions::dismiss(&mut tx, authed.user.auth.user.id, id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
