//! Dashboards (design doc §8.12): prebuilt ones, shared ones at an
//! organization (`dashboard:manage` there to change them), and personal ones
//! (`dashboard:read` is enough). Widgets' numbers come from
//! `/api/v1/analytics/query`, each with the viewer's own access.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use huxley_analytics::dashboards::{self, Dashboard, DashboardInput};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::analytics_routes::map;
use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dashboards", get(list_dashboards).post(create_dashboard))
        .route(
            "/dashboards/{id}",
            get(get_dashboard)
                .put(update_dashboard)
                .delete(delete_dashboard),
        )
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Where the caller can see dashboards; 403 when nowhere.
async fn read_scope(state: &AppState, authed: &Authed) -> ApiResult<OrgScope> {
    let scope = authed
        .permission_scope(state, permissions::DASHBOARD_READ, None)
        .await?;
    if matches!(&scope, OrgScope::Orgs(orgs) if orgs.is_empty()) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You can't see dashboards",
        ));
    }
    Ok(scope)
}

/// Prebuilt dashboards, then shared ones and your own.
#[utoipa::path(get, path = "/api/v1/dashboards", operation_id = "list_dashboards", tag = "analytics",
    responses((status = 200, body = Vec<Dashboard>)))]
pub async fn list_dashboards(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Dashboard>>> {
    let scope = read_scope(&state, &authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let stored = dashboards::list(&mut tx, authed.user.auth.user.id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let mut all = dashboards::builtin();
    all.extend(stored);
    Ok(Json(all))
}

/// One dashboard: a uuid, or a prebuilt one's key (e.g. `service-desk`).
#[utoipa::path(get, path = "/api/v1/dashboards/{id}", operation_id = "get_dashboard", tag = "analytics",
    params(("id" = String, Path)), responses((status = 200, body = Dashboard)))]
pub async fn get_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<String>,
) -> ApiResult<Json<Dashboard>> {
    let scope = read_scope(&state, &authed).await?;
    let Ok(uuid) = id.parse::<Uuid>() else {
        return dashboards::builtin()
            .into_iter()
            .find(|d| d.id == id)
            .map(Json)
            .ok_or_else(ApiError::not_found);
    };
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = dashboards::get(&mut tx, authed.user.auth.user.id, uuid)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    found.map(Json).ok_or_else(ApiError::not_found)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDashboard {
    /// Where it lives: shared there (and below), or personal.
    pub org_id: Uuid,
    /// Share it (needs `dashboard:manage` there); otherwise it's yours.
    #[serde(default)]
    pub shared: bool,
    pub dashboard: DashboardInput,
}

#[utoipa::path(post, path = "/api/v1/dashboards", operation_id = "create_dashboard", tag = "analytics",
    request_body = CreateDashboard,
    responses((status = 201, body = Dashboard), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateDashboard>,
) -> ApiResult<(StatusCode, Json<Dashboard>)> {
    let permission = if body.shared {
        permissions::DASHBOARD_MANAGE
    } else {
        permissions::DASHBOARD_READ
    };
    authed.require_at(&state, permission, body.org_id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![body.org_id]))
        .await
        .map_err(db_err)?;
    let owner = (!body.shared).then_some(authed.user.auth.user.id);
    let created = dashboards::create(
        &mut tx,
        &authed.user.ctx,
        body.org_id,
        owner,
        &body.dashboard,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// A stored dashboard the caller may change: their own, or shared where
/// they hold `dashboard:manage`.
async fn changeable(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<Dashboard> {
    let scope = read_scope(state, authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = dashboards::get(&mut tx, authed.user.auth.user.id, id)
        .await
        .map_err(map)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    if found.owner_user_id.is_none() {
        let org = found.org_id.ok_or_else(ApiError::not_found)?;
        authed
            .require_at(state, permissions::DASHBOARD_MANAGE, org)
            .await?;
    }
    Ok(found)
}

#[utoipa::path(put, path = "/api/v1/dashboards/{id}", operation_id = "update_dashboard", tag = "analytics",
    params(("id" = Uuid, Path)), request_body = DashboardInput,
    responses((status = 200, body = Dashboard), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DashboardInput>,
) -> ApiResult<Json<Dashboard>> {
    let current = changeable(&state, &authed, id).await?;
    let org = current.org_id.ok_or_else(ApiError::not_found)?;
    let sharing = body.shared == Some(true) && current.owner_user_id.is_some();
    if sharing {
        authed
            .require_at(&state, permissions::DASHBOARD_MANAGE, org)
            .await?;
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![org]))
        .await
        .map_err(db_err)?;
    if sharing {
        dashboards::share(&mut tx, &authed.user.ctx, id)
            .await
            .map_err(map)?;
    }
    let updated = dashboards::update(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    updated.map(Json).ok_or_else(|| {
        ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        )
    })
}

#[utoipa::path(delete, path = "/api/v1/dashboards/{id}", operation_id = "delete_dashboard", tag = "analytics",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let current = changeable(&state, &authed, id).await?;
    let org = current.org_id.ok_or_else(ApiError::not_found)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![org]))
        .await
        .map_err(db_err)?;
    dashboards::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
