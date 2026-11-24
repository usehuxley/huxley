//! `/api/v1/datasets` (design doc §9, ADR-029): dataset definitions and
//! their rows. Reading needs `dataset:read` and changing `dataset:manage`
//! at the dataset's organization; rows are further limited by row-level
//! security to organizations in the caller's scope.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_datasets::{Dataset, DatasetError, DatasetInput, Query, Rows};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .route("/{id}/rows", post(write_rows))
        .route("/{id}/rows/query", post(query_rows))
        .route("/{id}/rows/delete", post(delete_rows))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: DatasetError) -> ApiError {
    match e {
        DatasetError::NotFound => ApiError::not_found(),
        DatasetError::KeyTaken(_) => ApiError::conflict(e.to_string()),
        DatasetError::Invalid(field, message) => {
            ApiError::validation().with_field_error(field, message)
        }
        DatasetError::Db(e) => ApiError::internal(&e),
    }
}

async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<Dataset> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let dataset = huxley_datasets::get(&mut tx, id).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    authed.require_at(state, permission, dataset.org_id).await?;
    Ok(dataset)
}

/// Datasets you can see.
#[utoipa::path(get, path = "/api/v1/datasets", operation_id = "list_datasets", tag = "datasets",
    responses((status = 200, body = Vec<Dataset>)))]
pub async fn list(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<Vec<Dataset>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = huxley_datasets::list(&mut tx).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut paths = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let mut visible = Vec::new();
    for dataset in all {
        let path: Option<String> =
            sqlx::query_scalar("SELECT path::text FROM organizations WHERE id = $1")
                .bind(dataset.org_id)
                .fetch_optional(&mut *paths)
                .await
                .map_err(db_err)?;
        if path.is_some_and(|p| authed.principal.can_at_path(permissions::DATASET_READ, &p)) {
            visible.push(dataset);
        }
    }
    paths.commit().await.map_err(db_err)?;
    Ok(Json(visible))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDatasetRequest {
    pub org_id: Uuid,
    pub dataset: DatasetInput,
}

/// Creates a dataset and its table.
#[utoipa::path(post, path = "/api/v1/datasets", operation_id = "create_dataset", tag = "datasets",
    request_body = CreateDatasetRequest,
    responses((status = 201, body = Dataset), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateDatasetRequest>,
) -> ApiResult<(StatusCode, Json<Dataset>)> {
    authed
        .require_at(&state, permissions::DATASET_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let dataset = huxley_datasets::create(&mut tx, &authed.user.ctx, body.org_id, &body.dataset)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(dataset)))
}

#[utoipa::path(get, path = "/api/v1/datasets/{id}", operation_id = "get_dataset", tag = "datasets",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Dataset), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Dataset>> {
    Ok(Json(
        authorize(&state, &authed, permissions::DATASET_READ, id).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDatasetRequest {
    pub dataset: DatasetInput,
}

/// Changes a dataset's definition and migrates its table (a type change
/// whose values don't convert is refused, and nothing changes).
#[utoipa::path(put, path = "/api/v1/datasets/{id}", operation_id = "update_dataset", tag = "datasets",
    params(("id" = Uuid, Path)), request_body = UpdateDatasetRequest,
    responses((status = 200, body = Dataset), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDatasetRequest>,
) -> ApiResult<Json<Dataset>> {
    authorize(&state, &authed, permissions::DATASET_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let dataset = huxley_datasets::update(&mut tx, &authed.user.ctx, id, &body.dataset)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(dataset))
}

/// Deletes a dataset and all its rows.
#[utoipa::path(delete, path = "/api/v1/datasets/{id}", operation_id = "delete_dataset", tag = "datasets",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(&state, &authed, permissions::DATASET_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    huxley_datasets::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteRowsRequest {
    /// The organization the rows belong to: the dataset's or one below.
    pub org_id: Uuid,
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub rows: Vec<JsonValue>,
    /// Replace all of that organization's rows (a full refresh) instead of
    /// adding and updating.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WrittenRows {
    pub written: u64,
}

/// Adds rows (updating those whose key exists), or replaces an
/// organization's rows.
#[utoipa::path(post, path = "/api/v1/datasets/{id}/rows", operation_id = "write_dataset_rows", tag = "datasets",
    params(("id" = Uuid, Path)), request_body = WriteRowsRequest,
    responses((status = 200, body = WrittenRows), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn write_rows(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<WriteRowsRequest>,
) -> ApiResult<Json<WrittenRows>> {
    let dataset = authorize(&state, &authed, permissions::DATASET_MANAGE, id).await?;
    // Writing an organization's rows needs access there too.
    authed
        .require_at(&state, permissions::DATASET_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let written = if body.replace {
        huxley_datasets::replace(&mut tx, &dataset, body.org_id, &body.rows).await
    } else {
        huxley_datasets::upsert(&mut tx, &dataset, body.org_id, &body.rows).await
    }
    .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(WrittenRows { written }))
}

/// Rows matching equality filters (those of organizations in your scope).
#[utoipa::path(post, path = "/api/v1/datasets/{id}/rows/query", operation_id = "query_dataset_rows", tag = "datasets",
    params(("id" = Uuid, Path)), request_body = Query,
    responses((status = 200, body = Rows), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn query_rows(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Query>,
) -> ApiResult<Json<Rows>> {
    let dataset = authorize(&state, &authed, permissions::DATASET_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let rows = huxley_datasets::query(&mut tx, &dataset, &body)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(rows))
}

/// Deletes the rows matching equality filters (in your scope).
#[utoipa::path(post, path = "/api/v1/datasets/{id}/rows/delete", operation_id = "delete_dataset_rows", tag = "datasets",
    params(("id" = Uuid, Path)), request_body = Query,
    responses((status = 200, body = WrittenRows), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_rows(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Query>,
) -> ApiResult<Json<WrittenRows>> {
    let dataset = authorize(&state, &authed, permissions::DATASET_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let written = huxley_datasets::delete_rows(&mut tx, &dataset, &body)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(WrittenRows { written }))
}
