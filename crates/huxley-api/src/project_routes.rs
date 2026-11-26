//! Projects (ADR-056): projects with phases, milestones and tasks (tickets),
//! created blank or from templates; budget burn and health. Reading needs
//! `project:read`, working tasks and milestones `project:write`, and
//! creating or changing projects and templates `project:manage`, where
//! the project (or template) is.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::DeskError;
use huxley_servicedesk::project_templates::{self, Template, TemplateInput};
use huxley_servicedesk::projects::{self, NewProject, NewTask, Project, ProjectChanges};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::{ApiError, ApiResult, AppState};

/// The projects module.
pub const MODULE: &str = "projects";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list).post(create))
        .route("/projects/{id}", get(detail).patch(update))
        .route("/projects/{id}/tasks", post(add_task))
        .route("/projects/{id}/milestones/{milestone}", post(set_milestone))
        .route(
            "/project-templates",
            get(list_templates).post(create_template),
        )
        .route(
            "/project-templates/{id}",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
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

async fn org_of(state: &AppState, table: &str, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT org_id FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

async fn allowed(state: &AppState, authed: &Authed, org: Uuid, permission: &str) -> ApiResult<()> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilter {
    /// One company's projects.
    pub org_id: Option<Uuid>,
}

/// Projects you can see, newest first.
#[utoipa::path(get, path = "/api/v1/projects", operation_id = "list_projects", tag = "projects",
    params(ProjectFilter), responses((status = 200, body = Vec<Project>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(f): Query<ProjectFilter>,
) -> ApiResult<Json<Vec<Project>>> {
    let scope = authed
        .permission_scope(&state, permissions::PROJECT_READ, Some(MODULE))
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let all = projects::list(&mut tx, f.org_id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// Creates a project for a client company, laid out from a template if
/// given (its phases, milestones and task tickets, assigned as asked).
#[utoipa::path(post, path = "/api/v1/projects", operation_id = "create_project", tag = "projects",
    request_body = NewProject,
    responses((status = 201, body = Project), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<NewProject>,
) -> ApiResult<(StatusCode, Json<Project>)> {
    allowed(&state, &authed, body.org_id, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    let made = system(&state, async |tx| projects::create(tx, &ctx, &body).await).await?;
    Ok((StatusCode::CREATED, Json(made)))
}

/// A project: its budget burn, health, progress, phases, milestones and
/// tasks.
#[utoipa::path(get, path = "/api/v1/projects/{id}", operation_id = "get_project", tag = "projects",
    params(("id" = Uuid, Path)), responses((status = 200, body = Project)))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Project>> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_READ).await?;
    Ok(Json(
        system(&state, async |tx| projects::get(tx, id).await).await?,
    ))
}

/// Changes a project: its name, status, manager, end, budget, billing
/// method, budget thresholds and portal visibility.
#[utoipa::path(patch, path = "/api/v1/projects/{id}", operation_id = "update_project", tag = "projects",
    params(("id" = Uuid, Path)), request_body = ProjectChanges,
    responses((status = 200, body = Project), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ProjectChanges>,
) -> ApiResult<Json<Project>> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            projects::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Adds a task (a ticket) to a project.
#[utoipa::path(post, path = "/api/v1/projects/{id}/tasks", operation_id = "add_project_task", tag = "projects",
    params(("id" = Uuid, Path)), request_body = NewTask, responses((status = 201, body = Project)))]
pub async fn add_task(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<NewTask>,
) -> ApiResult<(StatusCode, Json<Project>)> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_WRITE).await?;
    authed
        .require_at(&state, permissions::TICKET_WRITE, org)
        .await?;
    let ctx = authed.user.ctx.clone();
    let p = system(&state, async |tx| {
        projects::add_task(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(p)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MilestoneState {
    pub reached: bool,
}

/// Marks a milestone reached (raising `project.milestone_reached`) or not.
#[utoipa::path(post, path = "/api/v1/projects/{id}/milestones/{milestone}", operation_id = "set_project_milestone", tag = "projects",
    params(("id" = Uuid, Path), ("milestone" = Uuid, Path)), request_body = MilestoneState, responses((status = 200, body = Project)))]
pub async fn set_milestone(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, milestone)): Path<(Uuid, Uuid)>,
    Json(body): Json<MilestoneState>,
) -> ApiResult<Json<Project>> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            projects::set_milestone(tx, &ctx, id, milestone, body.reached).await
        })
        .await?,
    ))
}

/// Project templates you can see.
#[utoipa::path(get, path = "/api/v1/project-templates", operation_id = "list_project_templates", tag = "projects",
    responses((status = 200, body = Vec<Template>)))]
pub async fn list_templates(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Template>>> {
    let scope = authed
        .permission_scope(&state, permissions::PROJECT_READ, Some(MODULE))
        .await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let all = project_templates::list(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

#[utoipa::path(get, path = "/api/v1/project-templates/{id}", operation_id = "get_project_template", tag = "projects",
    params(("id" = Uuid, Path)), responses((status = 200, body = Template)))]
pub async fn get_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Template>> {
    let org = org_of(&state, "prj_templates", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_READ).await?;
    Ok(Json(
        system(&state, async |tx| project_templates::get(tx, id).await).await?,
    ))
}

/// Adds a template: phases with tasks (estimates, dependencies, client
/// visibility) and milestones (days after the start).
#[utoipa::path(post, path = "/api/v1/project-templates", operation_id = "create_project_template", tag = "projects",
    request_body = TemplateInput,
    responses((status = 201, body = Template), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_template(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<TemplateInput>,
) -> ApiResult<(StatusCode, Json<Template>)> {
    allowed(&state, &authed, body.org_id, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    let t = system(&state, async |tx| {
        project_templates::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(t)))
}

#[utoipa::path(put, path = "/api/v1/project-templates/{id}", operation_id = "update_project_template", tag = "projects",
    params(("id" = Uuid, Path)), request_body = TemplateInput, responses((status = 200, body = Template)))]
pub async fn update_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TemplateInput>,
) -> ApiResult<Json<Template>> {
    let org = org_of(&state, "prj_templates", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            project_templates::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/project-templates/{id}", operation_id = "delete_project_template", tag = "projects",
    params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = org_of(&state, "prj_templates", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    if system(&state, async |tx| {
        project_templates::delete(tx, &ctx, id).await
    })
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
