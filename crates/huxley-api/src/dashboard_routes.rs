//! Dashboards (design doc §8.12): prebuilt ones, shared ones at an
//! organization (`dashboard:manage` there to change them), and personal ones
//! (`dashboard:read` is enough). Widgets' numbers come from
//! `/api/v1/analytics/query`, each with the viewer's own access.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_analytics::dashboards::{self, Dashboard, DashboardInput, Viewer};
use huxley_analytics::publishing::{self, Submission, SubmitInput};
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
        .route("/dashboards/{id}/copy", post(copy_dashboard))
        .route("/dashboards/{id}/submit", post(submit_dashboard))
        .route("/dashboards/{id}/submissions", get(dashboard_submissions))
        .route("/dashboard-submissions", get(pending_submissions))
        .route("/dashboard-submissions/{id}", get(get_submission))
        .route("/dashboard-audiences", get(list_audiences))
        .route(
            "/dashboard-submissions/{id}/approve",
            post(approve_submission),
        )
        .route(
            "/dashboard-submissions/{id}/reject",
            post(reject_submission),
        )
        .route(
            "/dashboard-submissions/{id}/withdraw",
            post(withdraw_submission),
        )
}

/// The caller as a dashboard viewer: their teams and roles, and whether
/// they manage dashboards anywhere.
async fn viewer(state: &AppState, authed: &Authed) -> ApiResult<Viewer> {
    let manage = authed
        .permission_scope(state, permissions::DASHBOARD_MANAGE, None)
        .await?;
    let manager = !matches!(&manage, OrgScope::Orgs(orgs) if orgs.is_empty());
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let v = Viewer::load(&mut tx, authed.user.auth.user.id, manager)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(v)
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
    let viewer = viewer(&state, &authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let stored = dashboards::list(&mut tx, &viewer).await.map_err(map)?;
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
    let viewer = viewer(&state, &authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = dashboards::get(&mut tx, &viewer, uuid).await.map_err(map)?;
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
    let viewer = viewer(state, authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = dashboards::get(&mut tx, &viewer, id)
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

/// A dashboard the caller can see: a prebuilt one's key, or a uuid.
async fn visible(state: &AppState, authed: &Authed, id: &str) -> ApiResult<Dashboard> {
    let scope = read_scope(state, authed).await?;
    let Ok(uuid) = id.parse::<Uuid>() else {
        return dashboards::builtin()
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(ApiError::not_found);
    };
    let viewer = viewer(state, authed).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let found = dashboards::get(&mut tx, &viewer, uuid).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    found.ok_or_else(ApiError::not_found)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopyDashboard {
    /// Where the copy lives (yours, personal).
    pub org_id: Uuid,
    /// Its name (default: "Copy of …").
    #[serde(default)]
    pub name: Option<String>,
}

/// Copies a dashboard you can see (prebuilt, shared or yours) into a new
/// personal one.
#[utoipa::path(post, path = "/api/v1/dashboards/{id}/copy", operation_id = "copy_dashboard", tag = "analytics",
    params(("id" = String, Path)), request_body = CopyDashboard,
    responses((status = 201, body = Dashboard)))]
pub async fn copy_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<String>,
    Json(body): Json<CopyDashboard>,
) -> ApiResult<(StatusCode, Json<Dashboard>)> {
    let source = visible(&state, &authed, &id).await?;
    authed
        .require_at(&state, permissions::DASHBOARD_READ, body.org_id)
        .await?;
    let name = body
        .name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            format!("Copy of {}", source.name)
                .chars()
                .take(100)
                .collect()
        });
    let input = DashboardInput {
        name,
        description: source.description,
        range: Some(source.range),
        widgets: source.widgets,
        portal: false,
        shared: None,
        version: None,
    };
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![body.org_id]))
        .await
        .map_err(db_err)?;
    let created = dashboards::create(
        &mut tx,
        &authed.user.ctx,
        body.org_id,
        Some(authed.user.auth.user.id),
        &input,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Submits your personal dashboard for publishing to an audience (teams,
/// roles, the client portal), as it is now. Someone with
/// `dashboard:manage` there approves or rejects it.
#[utoipa::path(post, path = "/api/v1/dashboards/{id}/submit", operation_id = "submit_dashboard", tag = "analytics",
    params(("id" = Uuid, Path)), request_body = SubmitInput,
    responses((status = 201, body = Submission), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn submit_dashboard(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SubmitInput>,
) -> ApiResult<(StatusCode, Json<Submission>)> {
    let current = visible(&state, &authed, &id.to_string()).await?;
    let org = current.org_id.ok_or_else(ApiError::not_found)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![org]))
        .await
        .map_err(db_err)?;
    let submitted = publishing::submit(
        &mut tx,
        &authed.user.ctx,
        authed.user.auth.user.id,
        id,
        &body,
    )
    .await
    .map_err(map)?
    .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(submitted)))
}

/// A dashboard's submissions and publications, newest first (its author,
/// or those who manage dashboards there).
#[utoipa::path(get, path = "/api/v1/dashboards/{id}/submissions", operation_id = "list_dashboard_submissions", tag = "analytics",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Submission>)))]
pub async fn dashboard_submissions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Submission>>> {
    let current = visible(&state, &authed, &id.to_string()).await?;
    let org = current.org_id.ok_or_else(ApiError::not_found)?;
    let mine = current.owner_user_id == Some(authed.user.auth.user.id)
        || current.created_by == Some(authed.user.auth.user.id);
    if !mine {
        authed
            .require_at(&state, permissions::DASHBOARD_MANAGE, org)
            .await?;
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![org]))
        .await
        .map_err(db_err)?;
    let all = publishing::for_dashboard(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// Submissions waiting for you: pending where you manage dashboards,
/// oldest first.
#[utoipa::path(get, path = "/api/v1/dashboard-submissions", operation_id = "list_pending_dashboard_submissions", tag = "analytics",
    responses((status = 200, body = Vec<Submission>)))]
pub async fn pending_submissions(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Submission>>> {
    let scope = authed
        .permission_scope(&state, permissions::DASHBOARD_MANAGE, None)
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let all = publishing::pending(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// One submission: its author's, or for those who manage dashboards there.
#[utoipa::path(get, path = "/api/v1/dashboard-submissions/{id}", operation_id = "get_dashboard_submission", tag = "analytics",
    params(("id" = Uuid, Path)), responses((status = 200, body = Submission)))]
pub async fn get_submission(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Submission>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = publishing::get(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let found = found.ok_or_else(ApiError::not_found)?;
    if found.submitted_by != Some(authed.user.auth.user.id) {
        authed
            .require_at(&state, permissions::DASHBOARD_MANAGE, found.org_id)
            .await
            .map_err(|_| ApiError::not_found())?;
    }
    Ok(Json(found))
}

/// The teams and roles a dashboard can be published to.
#[utoipa::path(get, path = "/api/v1/dashboard-audiences", operation_id = "list_dashboard_audiences", tag = "analytics",
    responses((status = 200, body = publishing::Audiences)))]
pub async fn list_audiences(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<publishing::Audiences>> {
    read_scope(&state, &authed).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let all = publishing::audiences(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideSubmission {
    /// Required to reject: what to change.
    #[serde(default)]
    pub comment: Option<String>,
}

async fn decide(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    approve: bool,
    comment: Option<&str>,
) -> ApiResult<Json<Submission>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = publishing::get(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let found = found.ok_or_else(ApiError::not_found)?;
    authed
        .require_at(state, permissions::DASHBOARD_MANAGE, found.org_id)
        .await?;
    if found.submitted_by == Some(authed.user.auth.user.id) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Someone else has to decide your own submission",
        ));
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::Orgs(vec![found.org_id]))
        .await
        .map_err(db_err)?;
    let decided = publishing::decide(&mut tx, &authed.user.ctx, id, approve, comment)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    decided
        .map(Json)
        .ok_or_else(|| ApiError::conflict("It has already been decided or withdrawn."))
}

/// Approves a submission: the dashboard is shared with the audience asked
/// for, as it was submitted.
#[utoipa::path(post, path = "/api/v1/dashboard-submissions/{id}/approve", operation_id = "approve_dashboard_submission", tag = "analytics",
    params(("id" = Uuid, Path)), request_body = DecideSubmission,
    responses((status = 200, body = Submission), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn approve_submission(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DecideSubmission>,
) -> ApiResult<Json<Submission>> {
    decide(&state, &authed, id, true, body.comment.as_deref()).await
}

/// Rejects a submission with a comment; the dashboard stays personal.
#[utoipa::path(post, path = "/api/v1/dashboard-submissions/{id}/reject", operation_id = "reject_dashboard_submission", tag = "analytics",
    params(("id" = Uuid, Path)), request_body = DecideSubmission,
    responses((status = 200, body = Submission), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn reject_submission(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<DecideSubmission>,
) -> ApiResult<Json<Submission>> {
    decide(&state, &authed, id, false, body.comment.as_deref()).await
}

/// Withdraws your pending submission.
#[utoipa::path(post, path = "/api/v1/dashboard-submissions/{id}/withdraw", operation_id = "withdraw_dashboard_submission", tag = "analytics",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn withdraw_submission(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let done = publishing::withdraw(&mut tx, &authed.user.ctx, authed.user.auth.user.id, id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    if done {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
