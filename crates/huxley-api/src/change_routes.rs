//! Change management (ADR-054): a change ticket's change details, submit,
//! implement and review; the impact of its affected assets; the change
//! calendar; freeze windows (`servicedesk:configure`). Reading needs
//! `ticket:read` where the ticket is, changing `ticket:write`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::DeskError;
use huxley_servicedesk::changes::{
    self, Calendar, Change, ChangeInput, Freeze, FreezeInput, RiskQuestion, Submitted,
};

/// The change management module.
const MODULE: &str = "change";
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/changes/questions", get(questions))
        .route("/changes/calendar", get(calendar))
        .route("/change-freezes", get(list_freezes).post(add_freeze))
        .route("/change-freezes/{id}", delete(delete_freeze))
        .route("/tickets/{id}/change", get(get_change).put(save_change))
        .route("/tickets/{id}/change/submit", post(submit))
        .route("/tickets/{id}/change/implemented", post(implemented))
        .route("/tickets/{id}/change/review", post(review))
        .route("/tickets/{id}/change/impact", get(impact))
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
        DeskError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        DeskError::Db(e) => ApiError::internal(&e),
        DeskError::Sqlx(e) => ApiError::internal(&e),
    }
}

/// The module is on where the ticket is, and the caller holds `permission`
/// there.
async fn allowed(
    state: &AppState,
    authed: &Authed,
    ticket: Uuid,
    permission: &str,
) -> ApiResult<()> {
    let org = crate::recommend_routes::ticket_org(state, ticket).await?;
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await
}

async fn system<T>(
    state: &AppState,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, DeskError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let v = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(v)
}

/// The risk questionnaire.
#[utoipa::path(get, path = "/api/v1/changes/questions", operation_id = "change_risk_questions", tag = "changes",
    responses((status = 200, body = Vec<RiskQuestion>)))]
pub async fn questions(_authed: Authed) -> Json<Vec<RiskQuestion>> {
    Json(changes::questions())
}

/// A change ticket's change details.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/change", operation_id = "get_change", tag = "changes",
    params(("id" = Uuid, Path)), responses((status = 200, body = Change)))]
pub async fn get_change(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Change>> {
    allowed(&state, &authed, id, permissions::TICKET_READ).await?;
    Ok(Json(
        system(&state, async |tx| changes::get(tx, id).await).await?,
    ))
}

/// Changes a draft or rejected change: its type, risk answers, plans,
/// window and affected assets.
#[utoipa::path(put, path = "/api/v1/tickets/{id}/change", operation_id = "save_change", tag = "changes",
    params(("id" = Uuid, Path)), request_body = ChangeInput,
    responses((status = 200, body = Change), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn save_change(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ChangeInput>,
) -> ApiResult<Json<Change>> {
    allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| changes::save(tx, &ctx, id, &body).await).await?,
    ))
}

/// Submits a change for approval (standard changes are approved at once).
/// Refused if its window falls in a freeze, unless it's an emergency.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/change/submit", operation_id = "submit_change", tag = "changes",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Submitted), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Submitted>> {
    allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| changes::submit(tx, &ctx, id).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ChangeOutcome)]
pub struct Outcome {
    /// `successful`, `failed` or `backed_out`.
    pub outcome: String,
}

/// Records that an approved change was carried out, and how it went.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/change/implemented", operation_id = "change_implemented", tag = "changes",
    params(("id" = Uuid, Path)), request_body = Outcome, responses((status = 200, body = Change)))]
pub async fn implemented(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Outcome>,
) -> ApiResult<Json<Change>> {
    allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            changes::implemented(tx, &ctx, id, &body.outcome).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ChangeReview)]
pub struct Review {
    pub review: String,
}

/// The post-implementation review.
#[utoipa::path(post, path = "/api/v1/tickets/{id}/change/review", operation_id = "review_change", tag = "changes",
    params(("id" = Uuid, Path)), request_body = Review, responses((status = 200, body = Change)))]
pub async fn review(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Review>,
) -> ApiResult<Json<Change>> {
    allowed(&state, &authed, id, permissions::TICKET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            changes::review(tx, &ctx, id, &body.review).await
        })
        .await?,
    ))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AffectedAsset {
    pub id: Uuid,
    pub name: String,
    pub type_name: String,
    pub org_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeImpact {
    /// The assets the change touches.
    pub affected: Vec<AffectedAsset>,
    /// What depends on them (from the dependency map), nearest first.
    pub impacted: Vec<huxley_cmdb::relationships::Impacted>,
}

/// The change's affected assets and everything that depends on them,
/// among the assets you can see.
#[utoipa::path(get, path = "/api/v1/tickets/{id}/change/impact", operation_id = "change_impact", tag = "changes",
    params(("id" = Uuid, Path)), responses((status = 200, body = ChangeImpact)))]
pub async fn impact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ChangeImpact>> {
    allowed(&state, &authed, id, permissions::TICKET_READ).await?;
    let change = system(&state, async |tx| changes::get(tx, id).await).await?;
    let scope = authed
        .permission_scope(&state, permissions::ASSET_READ, Some(huxley_cmdb::MODULE))
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let mut affected = Vec::new();
    let mut impacted: Vec<huxley_cmdb::relationships::Impacted> = Vec::new();
    for asset in &change.affected_assets {
        let Ok(a) = huxley_cmdb::assets::get(&mut tx, *asset).await else {
            continue;
        };
        affected.push(AffectedAsset {
            id: a.id,
            name: a.name,
            type_name: a.type_name,
            org_name: a.org_name,
        });
        for hit in huxley_cmdb::relationships::impact(&mut tx, *asset, 5)
            .await
            .unwrap_or_default()
        {
            if !change.affected_assets.contains(&hit.id) && !impacted.iter().any(|i| i.id == hit.id)
            {
                impacted.push(hit);
            }
        }
    }
    tx.commit().await.map_err(db_err)?;
    impacted.sort_by_key(|i| i.depth);
    Ok(Json(ChangeImpact { affected, impacted }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Window {
    /// RFC 3339 (default: now).
    pub from: Option<String>,
    /// RFC 3339 (default: four weeks after `from`).
    pub to: Option<String>,
}

fn when(value: Option<&String>, field: &str) -> ApiResult<Option<OffsetDateTime>> {
    value
        .map(|v| {
            OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339).map_err(|_| {
                ApiError::validation().with_field_error(format!("/{field}"), "an RFC 3339 time")
            })
        })
        .transpose()
}

/// Changes planned and freezes in a window, among what you can see.
#[utoipa::path(get, path = "/api/v1/changes/calendar", operation_id = "change_calendar", tag = "changes",
    params(Window), responses((status = 200, body = Calendar)))]
pub async fn calendar(
    State(state): State<AppState>,
    authed: Authed,
    Query(w): Query<Window>,
) -> ApiResult<Json<Calendar>> {
    let from = when(w.from.as_ref(), "from")?.unwrap_or_else(OffsetDateTime::now_utc);
    let to = when(w.to.as_ref(), "to")?.unwrap_or(from + time::Duration::weeks(4));
    let scope = authed
        .permission_scope(&state, permissions::TICKET_READ, Some(MODULE))
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = changes::calendar(&mut tx, from, to).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(found))
}

/// Freezes ending from now on, soonest first.
#[utoipa::path(get, path = "/api/v1/change-freezes", operation_id = "list_change_freezes", tag = "changes",
    responses((status = 200, body = Vec<Freeze>)))]
pub async fn list_freezes(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Freeze>>> {
    let scope = authed
        .permission_scope(&state, permissions::TICKET_READ, Some(MODULE))
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = changes::freezes(&mut tx, OffsetDateTime::now_utc())
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(found))
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = FreezeCreated)]
pub struct Created {
    pub id: Uuid,
}

/// Adds a freeze window at an organization (for it and those below).
#[utoipa::path(post, path = "/api/v1/change-freezes", operation_id = "add_change_freeze", tag = "changes",
    request_body = FreezeInput,
    responses((status = 201, body = Created), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_freeze(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<FreezeInput>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    require_module_at(&state, MODULE, body.org_id).await?;
    let ctx = authed.user.ctx.clone();
    let id = system(&state, async |tx| {
        changes::add_freeze(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(Created { id })))
}

#[utoipa::path(delete, path = "/api/v1/change-freezes/{id}", operation_id = "delete_change_freeze", tag = "changes",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_freeze(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SERVICEDESK_CONFIGURE)?;
    let ctx = authed.user.ctx.clone();
    if system(&state, async |tx| {
        changes::delete_freeze(tx, &ctx, id).await
    })
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
