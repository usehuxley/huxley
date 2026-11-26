//! `/api/v1/workflow-folders`: folders to organize workflows, and moving
//! workflows between them. Reading needs `workflow:read` and changing
//! `workflow:write`, at the folder's organization. Each organization's
//! System folder holds its built-in workflows and stays as it is.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::begin_scoped;
use huxley_workflows::folders::{self, Folder, FolderInput};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::workflow_routes::{map_workflow, org_paths};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_folders).post(create_folder))
        .route("/move", post(move_workflows))
        .route("/{id}", patch(update_folder).delete(delete_folder))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// The folder, checking `permission` at its organization.
async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<Folder> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let folder = folders::get(&mut tx, id).await.map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    authed.require_at(state, permission, folder.org_id).await?;
    Ok(folder)
}

/// Every folder the caller can view, with how many workflows each holds.
#[utoipa::path(get, path = "/api/v1/workflow-folders", operation_id = "list_workflow_folders",
    tag = "workflows", responses((status = 200, body = Vec<Folder>)))]
pub async fn list_folders(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Folder>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = folders::list(&mut tx).await.map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    let paths = org_paths(&state, all.iter().map(|f| f.org_id).collect()).await?;
    Ok(Json(
        all.into_iter()
            .filter(|f| {
                paths
                    .get(&f.org_id)
                    .is_some_and(|p| authed.principal.can_at_path(permissions::WORKFLOW_READ, p))
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateFolderRequest {
    pub org_id: Uuid,
    pub folder: FolderInput,
}

/// Makes a folder.
#[utoipa::path(post, path = "/api/v1/workflow-folders", operation_id = "create_workflow_folder",
    tag = "workflows", request_body = CreateFolderRequest,
    responses(
        (status = 201, body = Folder),
        (status = 409, description = "The name is taken there, or the System folder", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create_folder(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateFolderRequest>,
) -> ApiResult<(StatusCode, Json<Folder>)> {
    authed
        .require_at(&state, permissions::WORKFLOW_WRITE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let folder = folders::create(&mut tx, &authed.user.ctx, body.org_id, &body.folder)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(folder)))
}

/// Renames or moves a folder. The System folder stays as it is.
#[utoipa::path(patch, path = "/api/v1/workflow-folders/{id}", operation_id = "update_workflow_folder",
    tag = "workflows", params(("id" = Uuid, Path)), request_body = FolderInput,
    responses(
        (status = 200, body = Folder),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn update_folder(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<FolderInput>,
) -> ApiResult<Json<Folder>> {
    authorize(&state, &authed, permissions::WORKFLOW_WRITE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let folder = folders::update(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(folder))
}

/// Deletes a folder; what was in it moves up to its parent. The System
/// folder can't be deleted.
#[utoipa::path(delete, path = "/api/v1/workflow-folders/{id}", operation_id = "delete_workflow_folder",
    tag = "workflows", params(("id" = Uuid, Path)),
    responses(
        (status = 204),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn delete_folder(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(&state, &authed, permissions::WORKFLOW_WRITE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    folders::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_workflow)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveWorkflowsRequest {
    /// 1 to 500 workflows, all moved or none.
    pub workflow_ids: Vec<Uuid>,
    /// Where to: a folder of each workflow's organization, or none
    /// (unfiled).
    #[serde(default)]
    pub folder_id: Option<Uuid>,
}

/// Moves workflows into a folder (or out of any). Built-in workflows stay
/// in the System folder, which holds nothing else.
#[utoipa::path(post, path = "/api/v1/workflow-folders/move", operation_id = "move_workflows",
    tag = "workflows", request_body = MoveWorkflowsRequest,
    responses(
        (status = 204),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn move_workflows(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<MoveWorkflowsRequest>,
) -> ApiResult<StatusCode> {
    if body.workflow_ids.is_empty() || body.workflow_ids.len() > 500 {
        return Err(ApiError::validation().with_field_error("workflowIds", "1 to 500 workflows"));
    }
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    for id in &body.workflow_ids {
        let workflow = huxley_workflows::summary(&mut tx, *id)
            .await
            .map_err(map_workflow)?;
        authed
            .require_at(&state, permissions::WORKFLOW_WRITE, workflow.org_id)
            .await?;
        folders::move_workflow(&mut tx, &authed.user.ctx, *id, body.folder_id)
            .await
            .map_err(map_workflow)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
