//! `/api/v1/workflows` and `/api/v1/runs`: workflow definitions and runs.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::Stream;
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::runs::{self, PayloadField, RunDetail, RunFilter, RunSummary};
use huxley_engine::{RunStatus, StartRun};
use huxley_workflows::{
    DiagnosticView, Version, VersionSummary, WorkflowDetail, WorkflowError, WorkflowStatus,
    WorkflowSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn workflow_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/validate", post(validate))
        .route("/{id}", get(get_one))
        .route("/{id}/draft", put(save_draft))
        .route("/{id}/publish", post(publish))
        .route("/{id}/template", post(save_as_template))
        .route("/{id}/preview", post(preview_expression))
        .route("/{id}/archive", post(archive))
        .route("/{id}/restore", post(restore))
        .route("/{id}/run-as", put(set_run_as))
        .route("/{id}/versions", get(versions))
        .route("/{id}/versions/{version}", get(version))
        .route("/{id}/runs", post(start_run))
        .route("/{id}/triggers", get(triggers))
}

pub fn job_routes() -> Router<AppState> {
    Router::new()
        .route("/dead", get(dead_jobs))
        .route("/{id}/retry", post(retry_job))
}

pub fn run_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_runs))
        .route("/{id}", get(get_run))
        .route("/{id}/stream", get(stream_run))
        .route("/{id}/cancel", post(cancel_run))
        .route("/{id}/resume", post(resume_run))
        .route("/{id}/steps/{key}/{attempt}/{field}", get(step_payload))
        .route("/{id}/steps/{key}/{attempt}/script", get(step_script))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

pub(crate) fn map_workflow(e: WorkflowError) -> ApiError {
    match e {
        WorkflowError::NotFound | WorkflowError::VersionNotFound => ApiError::not_found(),
        WorkflowError::KeyTaken(_) | WorkflowError::Archived => ApiError::conflict(e.to_string()),
        WorkflowError::RevisionConflict(_) => ApiError::new(
            StatusCode::CONFLICT,
            "revision-conflict",
            "The draft changed",
        )
        .with_detail(e.to_string()),
        WorkflowError::InvalidDocument(detail) => ApiError::validation().with_detail(detail),
        WorkflowError::Invalid(diagnostics) => diagnostics.into_iter().fold(
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "workflow-invalid",
                "The workflow has problems",
            ),
            |error, d| error.with_field_error(d.path, d.message),
        ),
        WorkflowError::InvalidInputs(problems) => problems.into_iter().fold(
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "inputs-invalid",
                "The inputs don't match the workflow's input schema",
            ),
            |error, p| error.with_field_error("inputs", p),
        ),
        WorkflowError::InvalidRunAs => {
            ApiError::validation().with_field_error("serviceAccountId", e.to_string())
        }
        WorkflowError::Db(e) => ApiError::internal(&e),
    }
}

/// Paths of the given orgs, to check permissions on many rows at once.
async fn org_paths(state: &AppState, orgs: Vec<Uuid>) -> ApiResult<HashMap<Uuid, String>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, path::text FROM organizations WHERE id = ANY($1)")
            .bind(orgs)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

/// Problems with the draft's `integration.action` steps: integrations and
/// actions that don't exist for its organization. Looked up as the system,
/// since integrations usually live in an organization above.
async fn action_problems(
    state: &AppState,
    org_id: Uuid,
    document: &JsonValue,
) -> ApiResult<Vec<huxley_hwf::Diagnostic>> {
    // A document that doesn't parse has its own diagnostics already.
    let Ok(workflow) = serde_json::from_value::<huxley_hwf::Workflow>(document.clone()) else {
        return Ok(Vec::new());
    };
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let problems = huxley_integrations::usage::check_actions(&mut tx, org_id, &workflow)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(problems)
}

async fn with_action_problems(
    state: &AppState,
    mut detail: WorkflowDetail,
) -> ApiResult<WorkflowDetail> {
    let problems = action_problems(state, detail.summary.org_id, &detail.draft).await?;
    detail.diagnostics.extend(problems);
    Ok(detail)
}

/// The workflow's org, checking `permission` there (404 if not visible).
async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<WorkflowSummary> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let workflow = huxley_workflows::summary(&mut tx, id)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    authed
        .require_at(state, permission, workflow.org_id)
        .await?;
    Ok(workflow)
}

/// Workflows the caller can view.
#[utoipa::path(get, path = "/api/v1/workflows", operation_id = "list_workflows", tag = "workflows",
    responses((status = 200, body = Vec<WorkflowSummary>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<WorkflowSummary>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = huxley_workflows::list(&mut tx)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    let paths = org_paths(&state, all.iter().map(|w| w.org_id).collect()).await?;
    Ok(Json(
        all.into_iter()
            .filter(|w| {
                paths
                    .get(&w.org_id)
                    .is_some_and(|p| authed.principal.can_at_path(permissions::WORKFLOW_READ, p))
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowRequest {
    pub org_id: Uuid,
    /// An HWF document. It may be incomplete, but needs `metadata.key` and
    /// `metadata.name`.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub document: JsonValue,
}

/// Creates a workflow from a draft document.
#[utoipa::path(post, path = "/api/v1/workflows", operation_id = "create_workflow", tag = "workflows",
    request_body = CreateWorkflowRequest,
    responses(
        (status = 201, body = WorkflowDetail),
        (status = 409, description = "Key already used in the org", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateWorkflowRequest>,
) -> ApiResult<(StatusCode, Json<WorkflowDetail>)> {
    authed
        .require_at(&state, permissions::WORKFLOW_WRITE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = huxley_workflows::create(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        body.org_id,
        body.document,
    )
    .await
    .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(with_action_problems(&state, detail).await?),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ValidateRequest {
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub document: JsonValue,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ValidateResponse {
    /// Empty when the document is ready to publish.
    pub diagnostics: Vec<DiagnosticView>,
}

/// Checks a document without saving it.
#[utoipa::path(post, path = "/api/v1/workflows/validate", operation_id = "validate_workflow", tag = "workflows",
    request_body = ValidateRequest,
    responses((status = 200, body = ValidateResponse)))]
pub async fn validate(
    State(state): State<AppState>,
    _authed: Authed,
    Json(body): Json<ValidateRequest>,
) -> Json<ValidateResponse> {
    let diagnostics = huxley_workflows::diagnose(state.engine.expr(), &body.document)
        .into_iter()
        .map(|d| DiagnosticView {
            path: d.path,
            message: d.message,
        })
        .collect();
    Json(ValidateResponse { diagnostics })
}

#[utoipa::path(get, path = "/api/v1/workflows/{id}", operation_id = "get_workflow", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = WorkflowDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_one(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WorkflowDetail>> {
    authorize(&state, &authed, permissions::WORKFLOW_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = huxley_workflows::get(&mut tx, state.engine.expr(), id)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(with_action_problems(&state, detail).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SaveDraftRequest {
    /// The revision this edit is based on.
    pub revision: i32,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub document: JsonValue,
}

/// Saves the draft. Fails with 409 if someone else saved since `revision`.
#[utoipa::path(put, path = "/api/v1/workflows/{id}/draft", operation_id = "save_workflow_draft", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = SaveDraftRequest,
    responses(
        (status = 200, body = WorkflowDetail),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn save_draft(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SaveDraftRequest>,
) -> ApiResult<Json<WorkflowDetail>> {
    authorize(&state, &authed, permissions::WORKFLOW_WRITE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = huxley_workflows::save_draft(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        id,
        body.revision,
        body.document,
    )
    .await
    .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(with_action_problems(&state, detail).await?))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct PublishRequest {
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpressionPreviewRequest {
    /// The expression, as written in the workflow, e.g. `"{{ steps.a.output.id }}"`.
    #[schema(value_type = Object)]
    pub expression: JsonValue,
    /// A run of this workflow to evaluate against. Default: the latest.
    #[serde(default)]
    pub run_id: Option<Uuid>,
}

/// Evaluates an expression against a run of the workflow (the latest by
/// default), as a top-level step of it would see it. Secret values stay
/// redacted, and `secrets.*` isn't available.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/preview", operation_id = "preview_workflow_expression", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = ExpressionPreviewRequest,
    responses((status = 200, body = huxley_engine::Preview),
              (status = 404, description = "No run to preview with", body = ApiError, content_type = "application/problem+json")))]
pub async fn preview_expression(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ExpressionPreviewRequest>,
) -> ApiResult<Json<huxley_engine::Preview>> {
    authorize(&state, &authed, permissions::WORKFLOW_WRITE, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let run_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM runs WHERE workflow_id = $1 AND ($2::uuid IS NULL OR id = $2) \
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(id)
    .bind(body.run_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    let no_run = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no-run",
            "There's no run to preview with",
        )
        .with_detail("Run it once (a test run is enough) to preview expressions with its data.")
    };
    let run_id = run_id.ok_or_else(no_run)?;
    let preview = state
        .engine
        .preview_expression(&mut tx, run_id, &body.expression)
        .await
        .map_err(db_err)?
        .ok_or_else(no_run)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(preview))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveTemplateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Where it's shown in the gallery (default "Your templates").
    #[serde(default)]
    pub category: String,
}

/// Saves the workflow's draft as a template of its organization, offered
/// there and below. If a form starts it, the form comes along, and each
/// use of the template gets its own copy.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/template", operation_id = "save_workflow_as_template", tag = "templates",
    params(("id" = Uuid, Path)), request_body = SaveTemplateRequest,
    responses((status = 201, body = huxley_templates::TemplateInfo),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn save_as_template(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SaveTemplateRequest>,
) -> ApiResult<(StatusCode, Json<huxley_templates::TemplateInfo>)> {
    authorize(&state, &authed, permissions::WORKFLOW_PUBLISH, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let workflow = huxley_workflows::get(&mut tx, state.engine.expr(), id)
        .await
        .map_err(map_workflow)?;
    let org_id = workflow.summary.org_id;
    let form_key = workflow.draft["spec"]["triggers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["type"] == "form")
        .and_then(|t| t["form"].as_str())
        .map(str::to_owned);
    let form = match &form_key {
        Some(key) => sqlx::query_as::<_, (String, JsonValue, JsonValue)>(
            "SELECT name, schema, ui FROM forms WHERE org_id = $1 AND key = $2",
        )
        .bind(org_id)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .map(|(name, schema, ui)| huxley_templates::FormPart { name, schema, ui }),
        None => None,
    };
    let info = huxley_templates::TemplateInfo {
        key: String::new(),
        name: body.name,
        description: body.description,
        category: body.category,
        bindings: Vec::new(),
        has_form: false,
        org_id: Some(org_id),
    };
    let template =
        huxley_templates::from_workflow(info, workflow.draft, form_key.as_deref().zip(form));
    let saved = huxley_templates::store::save(&mut tx, &authed.user.ctx, org_id, &template)
        .await
        .map_err(|e| match e {
            huxley_templates::store::StoreError::Invalid(at, message) => {
                ApiError::validation().with_field_error(at, message)
            }
            other => ApiError::internal(&other),
        })?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(saved)))
}

/// Publishes the draft as a new version. 422 lists its problems if it has any.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/publish", operation_id = "publish_workflow", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = PublishRequest,
    responses(
        (status = 200, body = Version),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn publish(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<PublishRequest>,
) -> ApiResult<Json<Version>> {
    authorize(&state, &authed, permissions::WORKFLOW_PUBLISH, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    // Every action step must name an integration and action that exist.
    let draft = huxley_workflows::get(&mut tx, state.engine.expr(), id)
        .await
        .map_err(map_workflow)?;
    let problems = action_problems(&state, draft.summary.org_id, &draft.draft).await?;
    if !problems.is_empty() {
        return Err(map_workflow(WorkflowError::Invalid(problems)));
    }
    let version = huxley_workflows::publish(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        id,
        body.notes.as_deref(),
    )
    .await
    .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(version))
}

async fn set_archived(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    archived: bool,
) -> ApiResult<Json<WorkflowSummary>> {
    authorize(state, authed, permissions::WORKFLOW_PUBLISH, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let summary = huxley_workflows::set_archived(&mut tx, &authed.user.ctx, id, archived)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(summary))
}

/// Archives a workflow: it can't be edited or run until restored.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/archive", operation_id = "archive_workflow", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = WorkflowSummary)))]
pub async fn archive(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WorkflowSummary>> {
    set_archived(&state, &authed, id, true).await
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAsRequest {
    /// An active service account, or null for no identity.
    pub service_account_id: Option<Uuid>,
}

/// Sets the service account that runs started by a schedule, webhook or
/// event act as. Needs `workflow:publish` on the workflow and
/// `service_account:manage`: whoever hands out an account's identity could
/// already act as it.
#[utoipa::path(put, path = "/api/v1/workflows/{id}/run-as", operation_id = "set_workflow_run_as", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = RunAsRequest,
    responses((status = 200, body = WorkflowSummary), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_run_as(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<RunAsRequest>,
) -> ApiResult<Json<WorkflowSummary>> {
    authorize(&state, &authed, permissions::WORKFLOW_PUBLISH, id).await?;
    authed.require_instance(permissions::SERVICE_ACCOUNT_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let summary =
        huxley_workflows::set_run_as(&mut tx, &authed.user.ctx, id, body.service_account_id)
            .await
            .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(summary))
}

#[utoipa::path(post, path = "/api/v1/workflows/{id}/restore", operation_id = "restore_workflow", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = WorkflowSummary)))]
pub async fn restore(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<WorkflowSummary>> {
    set_archived(&state, &authed, id, false).await
}

#[utoipa::path(get, path = "/api/v1/workflows/{id}/versions", operation_id = "list_workflow_versions", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<VersionSummary>)))]
pub async fn versions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<VersionSummary>>> {
    authorize(&state, &authed, permissions::WORKFLOW_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let versions = huxley_workflows::versions(&mut tx, id)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(versions))
}

#[utoipa::path(get, path = "/api/v1/workflows/{id}/versions/{version}", operation_id = "get_workflow_version", tag = "workflows",
    params(("id" = Uuid, Path), ("version" = i32, Path)),
    responses((status = 200, body = Version), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn version(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, number)): Path<(Uuid, i32)>,
) -> ApiResult<Json<Version>> {
    authorize(&state, &authed, permissions::WORKFLOW_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let version = huxley_workflows::version(&mut tx, id, number)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(version))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartRunRequest {
    #[serde(default = "empty_object")]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub inputs: JsonValue,
    /// Run the draft instead of the latest published version (needs
    /// permission to edit the workflow).
    #[serde(default)]
    pub test: bool,
}

fn empty_object() -> JsonValue {
    json!({})
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StartRunResponse {
    pub id: Uuid,
}

/// Starts a manual run: of the latest published version, or a test run of
/// the draft.
#[utoipa::path(post, path = "/api/v1/workflows/{id}/runs", operation_id = "start_workflow_run", tag = "workflows",
    params(("id" = Uuid, Path)), request_body = StartRunRequest,
    responses(
        (status = 201, body = StartRunResponse),
        (status = 409, description = "Archived, or nothing published yet", body = ApiError, content_type = "application/problem+json"),
        (status = 422, description = "Invalid inputs or draft", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn start_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<StartRunRequest>,
) -> ApiResult<(StatusCode, Json<StartRunResponse>)> {
    let permission = if body.test {
        permissions::WORKFLOW_WRITE
    } else {
        permissions::WORKFLOW_RUN
    };
    let workflow = authorize(&state, &authed, permission, id).await?;
    if workflow.status == WorkflowStatus::Archived {
        return Err(map_workflow(WorkflowError::Archived));
    }
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let (version, document, run_document) = if body.test {
        let detail = huxley_workflows::get(&mut tx, state.engine.expr(), id)
            .await
            .map_err(map_workflow)?;
        if !detail.diagnostics.is_empty() {
            return Err(map_workflow(WorkflowError::Invalid(detail.diagnostics)));
        }
        (None, detail.draft.clone(), Some(detail.draft))
    } else {
        let Some(current) = workflow.current_version else {
            return Err(ApiError::conflict(
                "the workflow hasn't been published yet; publish it or start a test run",
            ));
        };
        let version = huxley_workflows::version(&mut tx, id, current)
            .await
            .map_err(map_workflow)?;
        (Some(current), version.document, None)
    };
    huxley_workflows::check_inputs(&document, &body.inputs).map_err(map_workflow)?;
    let run = state
        .engine
        .start_run(
            &mut tx,
            StartRun {
                org_id: workflow.org_id,
                workflow_id: Some(id),
                version,
                document: run_document,
                trigger_type: "manual".into(),
                trigger: json!({}),
                inputs: body.inputs,
                actor_id: authed.user.ctx.actor.id(),
            },
        )
        .await
        .map_err(db_err)?;
    huxley_db::audit::record(
        &mut tx,
        &authed.user.ctx,
        huxley_db::audit::AuditRecord::new("workflow.run_started")
            .org(workflow.org_id)
            .target("run", run)
            .details(json!({ "workflow": id, "version": version, "test": body.test })),
    )
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(StartRunResponse { id: run })))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct RunQuery {
    pub workflow_id: Option<Uuid>,
    pub status: Option<RunStatus>,
    /// Runs older than this run id (for paging).
    pub before: Option<Uuid>,
    /// At most this many (default 50, at most 200).
    pub limit: Option<i64>,
}

/// Runs the caller can view, newest first.
#[utoipa::path(get, path = "/api/v1/runs", operation_id = "list_runs", tag = "workflows",
    params(RunQuery), responses((status = 200, body = Vec<RunSummary>)))]
pub async fn list_runs(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<RunQuery>,
) -> ApiResult<Json<Vec<RunSummary>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let filter = RunFilter {
        workflow_id: query.workflow_id,
        status: query.status,
        before: query.before,
        limit: query.limit.unwrap_or(50),
    };
    let all = runs::list(&mut tx, &filter).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let paths = org_paths(&state, all.iter().map(|r| r.org_id).collect()).await?;
    Ok(Json(
        all.into_iter()
            .filter(|r| {
                paths
                    .get(&r.org_id)
                    .is_some_and(|p| authed.principal.can_at_path(permissions::WORKFLOW_READ, p))
            })
            .collect(),
    ))
}

async fn load_run(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<RunDetail> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let run = runs::get(&mut tx, id)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    authed
        .require_at(state, permissions::WORKFLOW_READ, run.summary.org_id)
        .await?;
    Ok(run)
}

/// A run with every step that has started.
#[utoipa::path(get, path = "/api/v1/runs/{id}", operation_id = "get_run", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = RunDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RunDetail>> {
    Ok(Json(load_run(&state, &authed, id).await?))
}

/// A run stream ends after this long; the browser reconnects.
const RUN_STREAM_LIFETIME: Duration = Duration::from_secs(600);
/// Changes closer together than this are sent as one update.
const RUN_STREAM_DEBOUNCE: Duration = Duration::from_millis(250);

/// Server-Sent Events: a `run` event with the run (as `GET /runs/{id}`
/// returns it) when the stream opens and after each change, at most four a
/// second. Ends once the run has finished, or after ten minutes (reconnect
/// to continue).
#[utoipa::path(get, path = "/api/v1/runs/{id}/stream", operation_id = "stream_run", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, description = "text/event-stream of `run` events", content_type = "text/event-stream"),
              (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn stream_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    // Subscribe first, so a change between the first read and here isn't missed.
    let rx = state.run_changes.subscribe();
    let first = load_run(&state, &authed, id).await?;
    let scope = authed.scope(&state).await?;
    let db = state.db.clone();
    let deadline = Instant::now() + RUN_STREAM_LIFETIME;

    let stream =
        futures_util::stream::unfold((rx, Some(first), false), move |(mut rx, pending, done)| {
            let (db, scope) = (db.clone(), scope.clone());
            async move {
                if done {
                    return None;
                }
                if let Some(run) = pending {
                    let finished = run.summary.status.is_finished();
                    let event = Event::default().event("run").json_data(&run).ok()?;
                    return Some((Ok(event), (rx, None, finished)));
                }
                // Wait for a change to this run.
                loop {
                    let remaining = deadline.checked_duration_since(Instant::now())?;
                    match tokio::time::timeout(remaining, rx.recv()).await {
                        Err(_) | Ok(Err(RecvError::Closed)) => return None,
                        Ok(Ok(changed)) if changed != id => {}
                        // Ours, or we fell behind and may have missed ours.
                        Ok(Ok(_) | Err(RecvError::Lagged(_))) => break,
                    }
                }
                // Let a burst of changes settle, then send the latest state.
                tokio::time::sleep(RUN_STREAM_DEBOUNCE).await;
                // (Skips what queued up meanwhile; lagging is fine here.)
                while !matches!(
                    rx.try_recv(),
                    Err(TryRecvError::Empty | TryRecvError::Closed)
                ) {}
                let mut tx = begin_scoped(&db, &scope).await.ok()?;
                let run = runs::get(&mut tx, id).await.ok()??;
                tx.commit().await.ok()?;
                let finished = run.summary.status.is_finished();
                let event = Event::default().event("run").json_data(&run).ok()?;
                Some((Ok(event), (rx, None, finished)))
            }
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CancelRunResponse {
    /// False if the run had already finished.
    pub cancelled: bool,
}

/// Asks a run to stop. Steps in flight finish; nothing new starts.
#[utoipa::path(post, path = "/api/v1/runs/{id}/cancel", operation_id = "cancel_run", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = CancelRunResponse)))]
pub async fn cancel_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<CancelRunResponse>> {
    let run = load_run(&state, &authed, id).await?;
    let permission = if run.summary.workflow_version.is_none() {
        permissions::WORKFLOW_WRITE
    } else {
        permissions::WORKFLOW_RUN
    };
    authed
        .require_at(&state, permission, run.summary.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let cancelled = state.engine.cancel_run(&mut tx, id).await.map_err(db_err)?;
    if cancelled {
        huxley_db::audit::record(
            &mut tx,
            &authed.user.ctx,
            huxley_db::audit::AuditRecord::new("workflow.run_cancelled")
                .org(run.summary.org_id)
                .target("run", id),
        )
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(Json(CancelRunResponse { cancelled }))
}

/// The published version's triggers: webhook paths and schedule times.
#[utoipa::path(get, path = "/api/v1/workflows/{id}/triggers", operation_id = "list_workflow_triggers", tag = "workflows",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<huxley_workflows::triggers::TriggerView>)))]
pub async fn triggers(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<huxley_workflows::triggers::TriggerView>>> {
    authorize(&state, &authed, permissions::WORKFLOW_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let triggers = huxley_workflows::triggers::list(&mut tx, id)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(triggers))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResumeRunResponse {
    /// False unless the run had failed.
    pub resumed: bool,
}

/// Resumes a failed run from the step that failed: steps that succeeded
/// aren't run again.
#[utoipa::path(post, path = "/api/v1/runs/{id}/resume", operation_id = "resume_run", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = ResumeRunResponse)))]
pub async fn resume_run(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ResumeRunResponse>> {
    let run = load_run(&state, &authed, id).await?;
    let permission = if run.summary.workflow_version.is_none() {
        permissions::WORKFLOW_WRITE
    } else {
        permissions::WORKFLOW_RUN
    };
    authed
        .require_at(&state, permission, run.summary.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let resumed = state.engine.resume_run(&mut tx, id).await.map_err(db_err)?;
    if resumed {
        huxley_db::audit::record(
            &mut tx,
            &authed.user.ctx,
            huxley_db::audit::AuditRecord::new("workflow.run_resumed")
                .org(run.summary.org_id)
                .target("run", id),
        )
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(Json(ResumeRunResponse { resumed }))
}

/// Workflow jobs that failed repeatedly and were set aside (dead letters).
/// A dead step job has already failed its step: resume the run instead.
#[utoipa::path(get, path = "/api/v1/jobs/dead", operation_id = "list_dead_jobs", tag = "workflows",
    responses((status = 200, body = Vec<huxley_engine::queue::DeadJob>)))]
pub async fn dead_jobs(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<huxley_engine::queue::DeadJob>>> {
    authed.require_instance(permissions::JOBS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let jobs = huxley_engine::queue::dead(&mut tx, 200)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(jobs))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RetryJobResponse {
    /// False if the job wasn't dead (or doesn't exist).
    pub retried: bool,
}

/// Puts a dead job back in the queue with fresh attempts.
#[utoipa::path(post, path = "/api/v1/jobs/{id}/retry", operation_id = "retry_job", tag = "workflows",
    params(("id" = Uuid, Path)), responses((status = 200, body = RetryJobResponse)))]
pub async fn retry_job(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<RetryJobResponse>> {
    authed.require_instance(permissions::JOBS_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let retried = huxley_engine::queue::retry_dead(&mut tx, id)
        .await
        .map_err(db_err)?;
    if retried {
        huxley_db::audit::record(
            &mut tx,
            &authed.user.ctx,
            huxley_db::audit::AuditRecord::new("workflow.job_retried").target("job", id),
        )
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(Json(RetryJobResponse { retried }))
}

/// A script step attempt's task: where it ran, how it ended, its logs.
#[utoipa::path(get, path = "/api/v1/runs/{id}/steps/{key}/{attempt}/script",
    operation_id = "get_step_script", tag = "workflows",
    params(("id" = Uuid, Path), ("key" = String, Path), ("attempt" = i32, Path)),
    responses((status = 200, body = huxley_engine::script::TaskView),
              (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn step_script(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key, attempt)): Path<(Uuid, String, i32)>,
) -> ApiResult<Json<huxley_engine::script::TaskView>> {
    // Checks the caller may read the run; the rest needs the system scope
    // (runner names).
    load_run(&state, &authed, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let view = huxley_engine::script::task_view(&mut tx, id, &key, attempt)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(view))
}

/// A step attempt's full input or output. Values over the spill threshold
/// are kept in object storage, and the run shows a `$spilled` marker in
/// their place; this returns the value itself either way.
#[utoipa::path(get, path = "/api/v1/runs/{id}/steps/{key}/{attempt}/{field}",
    operation_id = "get_step_payload", tag = "workflows",
    params(("id" = Uuid, Path), ("key" = String, Path, description = "Step key, e.g. `add_member[2]`"),
           ("attempt" = i32, Path), ("field" = PayloadField, Path)),
    responses((status = 200, body = serde_json::Value), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn step_payload(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key, attempt, field)): Path<(Uuid, String, i32, PayloadField)>,
) -> ApiResult<Json<JsonValue>> {
    // Checks the caller may read the run.
    load_run(&state, &authed, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let reference = runs::payload_ref(&mut tx, id, &key, attempt, field)
        .await
        .map_err(db_err)?
        .ok_or_else(ApiError::not_found)?;
    let inline: Option<JsonValue> = if reference.is_none() {
        let column = match field {
            PayloadField::Input => "input",
            PayloadField::Output => "output",
        };
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {column} FROM step_runs WHERE run_id = $1 AND step_key = $2 AND attempt = $3"
        )))
        .bind(id)
        .bind(&key)
        .bind(attempt)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?
    } else {
        None
    };
    tx.commit().await.map_err(db_err)?;
    match reference {
        Some(reference) => Ok(Json(
            state
                .engine
                .load_payload(&reference)
                .await
                .map_err(db_err)?,
        )),
        None => Ok(Json(inline.unwrap_or(JsonValue::Null))),
    }
}
