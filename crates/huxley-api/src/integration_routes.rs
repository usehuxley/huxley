//! `/api/v1/integrations`: APIs imported from OpenAPI specs (design doc
//! §7.1, ADR-023).

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::ImportError;
use huxley_integrations::store::{
    self, IntegrationDetail, IntegrationSummary, IntegrationVersion, StoreError,
};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

/// Largest request a spec import accepts: the spec plus JSON overhead.
pub const MAX_IMPORT_BYTES: usize = huxley_integrations::MAX_SPEC_BYTES + 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail))
        .route("/{id}/spec", put(update))
        .route("/{id}/versions", get(versions))
        .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found(),
        StoreError::KeyTaken(_) => ApiError::conflict(e.to_string()),
        StoreError::InvalidKey => ApiError::validation().with_field_error("key", e.to_string()),
        StoreError::Import(ImportError::TooLarge) => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "spec-too-large",
            "The spec is too large",
        ),
        StoreError::Import(e) => ApiError::validation().with_field_error("spec", e.to_string()),
        StoreError::Db(e) => ApiError::internal(&e),
    }
}

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

/// The integration, checking `permission` at its org (404 if not visible).
async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<IntegrationSummary> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let summary = store::summary(&mut tx, id).await.map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    authed.require_at(state, permission, summary.org_id).await?;
    Ok(summary)
}

/// Integrations the caller can view.
#[utoipa::path(get, path = "/api/v1/integrations", operation_id = "list_integrations", tag = "integrations",
    responses((status = 200, body = Vec<IntegrationSummary>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<IntegrationSummary>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = store::list(&mut tx).await.map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    let paths = org_paths(&state, all.iter().map(|i| i.org_id).collect()).await?;
    Ok(Json(
        all.into_iter()
            .filter(|i| {
                paths.get(&i.org_id).is_some_and(|p| {
                    authed
                        .principal
                        .can_at_path(permissions::INTEGRATION_READ, p)
                })
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    pub org_id: Uuid,
    /// Defaults to one made from the spec's title.
    pub key: Option<String>,
    /// The OpenAPI 3.x or Swagger 2.0 document, as JSON or YAML text.
    pub spec: String,
}

/// Imports a spec as a new integration. Problems the importer worked
/// around are returned as `warnings`; an unusable spec is a 422 on `spec`.
#[utoipa::path(post, path = "/api/v1/integrations", operation_id = "import_integration", tag = "integrations",
    request_body = ImportRequest,
    responses((status = 201, body = IntegrationDetail),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ImportRequest>,
) -> ApiResult<(StatusCode, Json<IntegrationDetail>)> {
    authed
        .require_at(&state, permissions::INTEGRATION_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = store::create(
        &mut tx,
        &authed.user.ctx,
        body.org_id,
        body.key.as_deref(),
        &body.spec,
    )
    .await
    .map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// An integration with its current definition.
#[utoipa::path(get, path = "/api/v1/integrations/{id}", operation_id = "get_integration", tag = "integrations",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = IntegrationDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<IntegrationDetail>> {
    authorize(&state, &authed, permissions::INTEGRATION_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = store::get(&mut tx, id).await.map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(detail))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSpecRequest {
    pub spec: String,
}

/// Imports a newer spec as the integration's next version.
#[utoipa::path(put, path = "/api/v1/integrations/{id}/spec", operation_id = "update_integration_spec", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = UpdateSpecRequest,
    responses((status = 200, body = IntegrationDetail), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSpecRequest>,
) -> ApiResult<Json<IntegrationDetail>> {
    authorize(&state, &authed, permissions::INTEGRATION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = store::update(&mut tx, &authed.user.ctx, id, &body.spec)
        .await
        .map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(detail))
}

/// Every imported version, newest first.
#[utoipa::path(get, path = "/api/v1/integrations/{id}/versions", operation_id = "list_integration_versions", tag = "integrations",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<IntegrationVersion>)))]
pub async fn versions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<IntegrationVersion>>> {
    authorize(&state, &authed, permissions::INTEGRATION_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let versions = store::versions(&mut tx, id).await.map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(versions))
}
