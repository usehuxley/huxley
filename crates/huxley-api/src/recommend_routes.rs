//! Dispatch recommendations (ADR-048): who should take a ticket, with the
//! reasons. Needs `ticket:read` where the ticket is; skills and
//! availability count while the workforce module is on there.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_workforce::WorkforceError;
use huxley_workforce::recommend::{self, Recommendation};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/tickets/{id}/recommendations", get(recommendations))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map(e: WorkforceError) -> ApiError {
    match e {
        WorkforceError::NotFound => ApiError::not_found(),
        other => ApiError::internal(&other),
    }
}

/// The organization a ticket belongs to.
pub(crate) async fn ticket_org(state: &AppState, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RecommendParams {
    /// How many (default 5, at most 20).
    pub limit: Option<usize>,
}

/// Who should take the ticket, best first: required skills and
/// certifications (its queue's, type's and its own), availability, open
/// load, history in its queue, and familiarity with the client, each with
/// reasons and cautions.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/recommendations", operation_id = "recommend_assignees", tag = "servicedesk",
    params(("id" = Uuid, Path), RecommendParams), responses((status = 200, body = Vec<Recommendation>)))]
pub async fn recommendations(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<RecommendParams>,
) -> ApiResult<Json<Vec<Recommendation>>> {
    let org = ticket_org(&state, id).await?;
    require_module_at(&state, huxley_servicedesk::MODULE, org).await?;
    authed
        .require_at(&state, permissions::TICKET_READ, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let workforce = huxley_modules::is_active_at(&mut tx, huxley_workforce::MODULE, org)
        .await
        .map_err(db_err)?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let all = recommend::for_ticket(
        &mut tx,
        id,
        now,
        workforce,
        params.limit.unwrap_or(5).clamp(1, 20),
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}
