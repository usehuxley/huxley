//! Satisfaction surveys (ADR-043).
//!
//! - The answer link (no sign-in; the token is the key):
//!   `/api/portal/v1/surveys/{token}`.
//! - Staff read a ticket's survey: `/api/v1/tickets/{id}/csat` (`ticket:read`).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_cx::nps;
use huxley_cx::surveys::{self, Survey};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::branding::Branding;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::portal_routes::PortalAuthed;
use crate::portal_routes::map as portal_error;
use crate::portal_ticket_routes::{PortalRating, cx_error};
use crate::{ApiError, ApiResult, AppState};

/// Portal routes (under `/api/portal/v1`), without a portal session.
pub fn portal_routes() -> Router<AppState> {
    Router::new()
        .route("/surveys/{token}", get(get_survey).post(answer_survey))
        .route("/nps/{token}", get(get_nps).post(answer_nps))
        .route("/me/nps", get(my_nps).post(answer_my_nps))
}

/// Staff routes (under `/api/v1`).
pub fn staff_routes() -> Router<AppState> {
    Router::new()
        .route("/tickets/{id}/csat", get(ticket_csat))
        .route("/tickets/{id}/sentiment", get(ticket_sentiment))
        .route("/contacts/{id}/nps", get(contact_nps))
        .route("/orgs/{id}/health", get(org_health))
        .route(
            "/orgs/{id}/health-model",
            get(get_health_model).put(set_health_model),
        )
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

/// A net promoter question as its answer link (or the portal) shows it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicNps {
    /// The company asking (the MSP's brand).
    pub company: String,
    pub score: Option<i16>,
    pub comment: Option<String>,
    pub open: bool,
    pub branding: Branding,
}

async fn public_nps(state: &AppState, survey: nps::Survey) -> ApiResult<PublicNps> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let branding = huxley_portal::branding::at(&mut tx, survey.org_id)
        .await
        .map_err(portal_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(PublicNps {
        open: survey.open(),
        company: branding.name.clone(),
        score: survey.score,
        comment: survey.comment,
        branding,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NpsAnswer {
    /// 0 (not at all likely) to 10 (extremely likely).
    pub score: i16,
    #[serde(default)]
    pub comment: Option<String>,
}

async fn nps_by_token(state: &AppState, token: &str) -> ApiResult<nps::Survey> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = nps::by_token(&mut tx, token).await.map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(survey)
}

/// The net promoter question an answer link is for.
#[utoipa::path(get, path = "/api/portal/v1/nps/{token}", operation_id = "get_nps_survey", tag = "portal",
    params(("token" = String, Path)), responses((status = 200, body = PublicNps), (status = 404)))]
pub async fn get_nps(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
) -> ApiResult<Json<PublicNps>> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let survey = nps_by_token(&state, &token).await?;
    Ok(Json(public_nps(&state, survey).await?))
}

/// Answers a net promoter question from its link (once).
#[utoipa::path(post, path = "/api/portal/v1/nps/{token}", operation_id = "answer_nps_survey", tag = "portal",
    params(("token" = String, Path)), request_body = NpsAnswer,
    responses((status = 200, body = PublicNps), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn answer_nps(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Path(token): Path<String>,
    Json(body): Json<NpsAnswer>,
) -> ApiResult<Json<PublicNps>> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let survey = nps_by_token(&state, &token).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let answered = nps::respond(
        &mut tx,
        &ctx,
        &survey,
        body.score,
        body.comment.as_deref(),
        "email",
    )
    .await
    .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(public_nps(&state, answered).await?))
}

/// Your net promoter question waiting for an answer, if any.
#[utoipa::path(get, path = "/api/portal/v1/me/nps", operation_id = "portal_my_nps", tag = "portal",
    responses((status = 200, body = Option<PublicNps>)))]
pub async fn my_nps(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Option<PublicNps>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = nps::open_for_contact(&mut tx, authed.principal.contact_id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(match survey {
        Some(s) => Some(public_nps(&state, s).await?),
        None => None,
    }))
}

/// Answers your waiting net promoter question.
#[utoipa::path(post, path = "/api/portal/v1/me/nps", operation_id = "portal_answer_my_nps", tag = "portal",
    request_body = NpsAnswer,
    responses((status = 200, body = PublicNps), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn answer_my_nps(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<NpsAnswer>,
) -> ApiResult<Json<PublicNps>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = nps::open_for_contact(&mut tx, authed.principal.contact_id)
        .await
        .map_err(cx_error)?
        .ok_or_else(ApiError::not_found)?;
    let answered = nps::respond(
        &mut tx,
        &authed.ctx,
        &survey,
        body.score,
        body.comment.as_deref(),
        "portal",
    )
    .await
    .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(public_nps(&state, answered).await?))
}

/// A contact's latest net promoter question and answer.
#[utoipa::path(get, path = "/api/v1/contacts/{id}/nps", operation_id = "get_contact_nps", tag = "crm",
    params(("id" = Uuid, Path)), responses((status = 200, body = Option<nps::Survey>)))]
pub async fn contact_nps(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Option<nps::Survey>>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM crm_contacts WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    authed
        .require_at(&state, permissions::CRM_READ, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let survey = nps::latest_for_contact(&mut tx, id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(survey))
}

/// A client company's health: its latest score, factors and reasons, and
/// its scores over the last 90 days. `null` before it's first scored.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/health", operation_id = "get_client_health", tag = "crm",
    params(("id" = Uuid, Path)), responses((status = 200, body = Option<huxley_cx::health::Health>)))]
pub async fn org_health(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Option<huxley_cx::health::Health>>> {
    authed.require_at(&state, permissions::CRM_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let health = huxley_cx::health::latest(&mut tx, id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(health))
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthModel {
    /// Each factor's weight, 0 (off) to 10: `csat`, `nps`, `sla`,
    /// `reopens`, `volume`, `renewal`.
    pub weights: huxley_cx::health::Weights,
}

/// The weights this organization's clients are scored with (set here or
/// above, else the defaults).
#[utoipa::path(get, path = "/api/v1/orgs/{id}/health-model", operation_id = "get_health_model", tag = "crm",
    params(("id" = Uuid, Path)), responses((status = 200, body = HealthModel)))]
pub async fn get_health_model(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<HealthModel>> {
    authed.require_at(&state, permissions::CRM_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let weights = huxley_cx::health::weights(&mut tx, id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(HealthModel { weights }))
}

/// Sets the weights for this organization's clients (and those below).
#[utoipa::path(put, path = "/api/v1/orgs/{id}/health-model", operation_id = "set_health_model", tag = "crm",
    params(("id" = Uuid, Path)), request_body = HealthModel,
    responses((status = 200, body = HealthModel), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_health_model(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<HealthModel>,
) -> ApiResult<Json<HealthModel>> {
    authed
        .require_at(&state, permissions::CRM_MANAGE, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let weights = huxley_cx::health::set_weights(&mut tx, &authed.user.ctx, id, &body.weights)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(HealthModel { weights }))
}

/// A ticket's judged client messages (the AI sentiment pack), oldest first.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/sentiment", operation_id = "get_ticket_sentiment", tag = "servicedesk",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<huxley_cx::sentiment::Entry>)))]
pub async fn ticket_sentiment(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<huxley_cx::sentiment::Entry>>> {
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
    let all = huxley_cx::sentiment::for_ticket(&mut tx, id)
        .await
        .map_err(cx_error)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}
