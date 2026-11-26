//! Projects (ADR-056): projects with phases, milestones and tasks (tickets),
//! created blank or from templates; budget burn and health. Reading needs
//! `project:read`, working tasks and milestones `project:write`, and
//! creating or changing projects and templates `project:manage`, where
//! the project (or template) is.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::DeskError;
use huxley_servicedesk::project_scope::{self, ScopeChange, ScopeChangeInput};
use huxley_servicedesk::project_templates::{self, Template, TemplateInput};
use huxley_servicedesk::projects::{
    self, NewProject, NewTask, Project, ProjectChanges, TaskChanges,
};
use serde::{Deserialize, Serialize};
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
        .route("/projects/capacity", get(capacity))
        .route("/projects/{id}", get(detail).patch(update))
        .route("/projects/{id}/tasks", post(add_task))
        .route("/projects/{id}/tasks/{ticket}", patch(update_task))
        .route("/projects/{id}/scope-changes", post(create_scope_change))
        .route(
            "/projects/{id}/scope-changes/{change}",
            put(update_scope_change),
        )
        .route(
            "/projects/{id}/scope-changes/{change}/submit",
            post(submit_scope_change),
        )
        .route(
            "/projects/{id}/scope-changes/{change}/withdraw",
            post(withdraw_scope_change),
        )
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
        .route("/project-templates/{id}/workflows", get(template_workflows))
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

/// Changes a task's phase, estimate, dates, dependencies or client
/// visibility.
#[utoipa::path(patch, path = "/api/v1/projects/{id}/tasks/{ticket}", operation_id = "update_project_task", tag = "projects",
    params(("id" = Uuid, Path), ("ticket" = Uuid, Path)), request_body = TaskChanges, responses((status = 200, body = Project)))]
pub async fn update_task(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, ticket)): Path<(Uuid, Uuid)>,
    Json(body): Json<TaskChanges>,
) -> ApiResult<Json<Project>> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_WRITE).await?;
    Ok(Json(
        system(&state, async |tx| {
            projects::update_task(tx, id, ticket, &body).await
        })
        .await?,
    ))
}

/// The scope change, checked to be the project's.
async fn scope_of(state: &AppState, authed: &Authed, id: Uuid, change: Uuid) -> ApiResult<()> {
    let org = org_of(state, "prj_projects", id).await?;
    allowed(state, authed, org, permissions::PROJECT_MANAGE).await?;
    let found = system(state, async |tx| project_scope::get(tx, change).await).await?;
    if found.project_id == id {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// Drafts a scope change: work added (or removed), in hours and
/// optionally priced.
#[utoipa::path(post, path = "/api/v1/projects/{id}/scope-changes", operation_id = "create_scope_change", tag = "projects",
    params(("id" = Uuid, Path)), request_body = ScopeChangeInput,
    responses((status = 201, body = ScopeChange), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_scope_change(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ScopeChangeInput>,
) -> ApiResult<(StatusCode, Json<ScopeChange>)> {
    let org = org_of(&state, "prj_projects", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_MANAGE).await?;
    let ctx = authed.user.ctx.clone();
    let made = system(&state, async |tx| {
        project_scope::create(tx, &ctx, id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(made)))
}

/// Changes a draft scope change.
#[utoipa::path(put, path = "/api/v1/projects/{id}/scope-changes/{change}", operation_id = "update_scope_change", tag = "projects",
    params(("id" = Uuid, Path), ("change" = Uuid, Path)), request_body = ScopeChangeInput, responses((status = 200, body = ScopeChange)))]
pub async fn update_scope_change(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, change)): Path<(Uuid, Uuid)>,
    Json(body): Json<ScopeChangeInput>,
) -> ApiResult<Json<ScopeChange>> {
    scope_of(&state, &authed, id, change).await?;
    Ok(Json(
        system(&state, async |tx| {
            project_scope::update(tx, change, &body).await
        })
        .await?,
    ))
}

/// Submits a draft for the client's approval (raising
/// `project.scope_change_submitted`).
#[utoipa::path(post, path = "/api/v1/projects/{id}/scope-changes/{change}/submit", operation_id = "submit_scope_change", tag = "projects",
    params(("id" = Uuid, Path), ("change" = Uuid, Path)), responses((status = 200, body = ScopeChange)))]
pub async fn submit_scope_change(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, change)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ScopeChange>> {
    scope_of(&state, &authed, id, change).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            project_scope::submit(tx, &ctx, change).await
        })
        .await?,
    ))
}

/// Withdraws a draft or submitted scope change.
#[utoipa::path(post, path = "/api/v1/projects/{id}/scope-changes/{change}/withdraw", operation_id = "withdraw_scope_change", tag = "projects",
    params(("id" = Uuid, Path), ("change" = Uuid, Path)), responses((status = 200, body = ScopeChange)))]
pub async fn withdraw_scope_change(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, change)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<ScopeChange>> {
    scope_of(&state, &authed, id, change).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            project_scope::withdraw(tx, &ctx, change).await
        })
        .await?,
    ))
}

/// A workflow that runs for one template's projects.
#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TemplateWorkflow {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    /// The project event it runs on.
    pub event: String,
}

/// Workflows for this template's projects: those with a `project.*` event
/// trigger whose filter names the template's id (e.g.
/// `{{ event.payload.templateId == '<id>' }}`).
#[utoipa::path(get, path = "/api/v1/project-templates/{id}/workflows", operation_id = "project_template_workflows", tag = "projects",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<TemplateWorkflow>)))]
pub async fn template_workflows(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<TemplateWorkflow>>> {
    let org = org_of(&state, "prj_templates", id).await?;
    allowed(&state, &authed, org, permissions::PROJECT_READ).await?;
    authed
        .require_at(&state, permissions::WORKFLOW_READ, org)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found: Vec<TemplateWorkflow> = sqlx::query_as(
        "SELECT DISTINCT w.id, w.name, w.status::text AS status, t.config->>'event' AS event \
           FROM workflow_triggers t JOIN workflows w ON w.id = t.workflow_id \
           JOIN organizations o ON o.id = w.org_id \
          WHERE t.type = 'event' AND t.config->>'event' LIKE 'project.%' \
            AND strpos(t.config->>'filter', $1) > 0 \
            AND o.path <@ (SELECT path FROM organizations WHERE id = $2) \
          ORDER BY w.name",
    )
    .bind(id.to_string())
    .bind(org)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(found))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CapacityParams {
    /// A day in the first week (`YYYY-MM-DD`; default this week).
    pub from: Option<String>,
    /// How many weeks (default 6, at most 26).
    pub weeks: Option<u8>,
    /// One team's members.
    pub team_id: Option<Uuid>,
}

/// Each person's hours a week against what's booked: appointments and the
/// remaining estimates of their project tasks, spread over the tasks'
/// dates. Over-booked weeks are flagged.
#[utoipa::path(get, path = "/api/v1/projects/capacity", operation_id = "project_capacity", tag = "projects",
    params(CapacityParams), responses((status = 200, body = huxley_servicedesk::capacity::Capacity)))]
pub async fn capacity(
    State(state): State<AppState>,
    authed: Authed,
    Query(p): Query<CapacityParams>,
) -> ApiResult<Json<huxley_servicedesk::capacity::Capacity>> {
    let scope = authed
        .permission_scope(&state, permissions::PROJECT_READ, Some(MODULE))
        .await?;
    let from = match p.from.as_deref().filter(|s| !s.is_empty()) {
        Some(d) => projects::parse_date(d, "from").map_err(map)?,
        None => time::OffsetDateTime::now_utc().date(),
    };
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let plan = huxley_servicedesk::capacity::plan(&mut tx, from, p.weeks.unwrap_or(6), p.team_id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(plan))
}
