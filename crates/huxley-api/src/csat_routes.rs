//! Satisfaction surveys (ADR-043).
//!
//! - The answer link (no sign-in; the token is the key):
//!   `/api/portal/v1/surveys/{token}`.
//! - Staff read a ticket's survey: `/api/v1/tickets/{id}/csat` (`ticket:read`).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_cx::surveys::{self, Survey};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::branding::Branding;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::portal_routes::map as portal_error;
use crate::portal_ticket_routes::{PortalRating, cx_error};
use crate::{ApiError, ApiResult, AppState};

/// Portal routes (under `/api/portal/v1`), without a portal session.
pub fn portal_routes() -> Router<AppState> {
    Router::new().route("/surveys/{token}", get(get_survey).post(answer_survey))
}

/// Staff routes (under `/api/v1`).
pub fn staff_routes() -> Router<AppState> {
    Router::new().route("/tickets/{id}/csat", get(ticket_csat))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// A survey as its answer link shows it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicSurvey {
    pub reference: String,
    pub title: String,
    /// `five` (1 to 5) or `thumbs` (1 down, 5 up).
    pub scale: String,
    pub rating: Option<i16>,
    pub comment: Option<String>,
    /// It can still be answered.
    pub open: bool,
    pub branding: Branding,
}

async fn public(state: &AppState, survey: Survey) -> ApiResult<PublicSurvey> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let branding = huxley_portal::branding::at(&mut tx, survey.org_id)
        .await
        .map_err(portal_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(PublicSurvey {
        open: survey.open(),
        reference: survey.reference,
        title: survey.title,
        scale: survey.scale,
        rating: survey.rating,
        comment: survey.comment,
        branding,
    })
}

async fn by_token(state: &AppState, token: &str) -> ApiResult<Survey> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = surveys::by_token(&mut tx, token).await.map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(survey)
}

/// The survey an answer link is for.
#[utoipa::path(get, path = "/api/portal/v1/surveys/{token}", operation_id = "get_csat_survey", tag = "portal",
    params(("token" = String, Path)), responses((status = 200, body = PublicSurvey), (status = 404)))]
pub async fn get_survey(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
) -> ApiResult<Json<PublicSurvey>> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let survey = by_token(&state, &token).await?;
    Ok(Json(public(&state, survey).await?))
}

/// Answers a survey from its link (once).
#[utoipa::path(post, path = "/api/portal/v1/surveys/{token}", operation_id = "answer_csat_survey", tag = "portal",
    params(("token" = String, Path)), request_body = PortalRating,
    responses((status = 200, body = PublicSurvey), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn answer_survey(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
    Json(body): Json<PortalRating>,
) -> ApiResult<Json<PublicSurvey>> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let survey = by_token(&state, &token).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let answered = surveys::respond(
        &mut tx,
        &ctx,
        &survey,
        body.rating,
        body.comment.as_deref(),
        "email",
    )
    .await
    .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(public(&state, answered).await?))
}

/// A ticket's latest satisfaction survey, if it has one.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/csat", operation_id = "get_ticket_csat", tag = "tickets",
    params(("id" = Uuid, Path)), responses((status = 200, body = Option<Survey>)))]
pub async fn ticket_csat(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Option<Survey>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    authed
        .require_at(&state, permissions::TICKET_READ, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = surveys::latest_for_ticket(&mut tx, id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(survey))
}
