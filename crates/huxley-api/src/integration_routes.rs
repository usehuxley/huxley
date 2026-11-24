//! `/api/v1/integrations`: APIs imported from OpenAPI specs (design doc
//! §7.1, ADR-023).

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_integrations::ImportError;
use huxley_integrations::store::{
    self, IntegrationDetail, IntegrationSummary, IntegrationVersion, StoreError, UpdatePreview,
};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

/// Largest request a spec import accepts: the spec plus JSON overhead.
pub const MAX_IMPORT_BYTES: usize = huxley_integrations::MAX_SPEC_BYTES + 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/actions", get(actions))
        .route("/catalog", get(catalog))
        .route("/catalog/{key}", post(add_built_in))
        .route("/{id}", get(detail))
        .route("/{id}/spec", put(update))
        .route("/{id}/preview", post(preview))
        .route("/{id}/current", put(set_current))
        .route("/{id}/overlay", put(set_overlay))
        .route("/{id}/versions", get(versions))
        .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound | StoreError::VersionNotFound(_) => ApiError::not_found(),
        StoreError::Stale(_) => ApiError::new(
            StatusCode::CONFLICT,
            "integration-changed",
            "The integration changed since the preview",
        )
        .with_detail(e.to_string()),
        StoreError::KeyTaken(_) => ApiError::conflict(e.to_string()),
        StoreError::InvalidKey => ApiError::validation().with_field_error("key", e.to_string()),
        StoreError::Invalid(at, message) => ApiError::validation().with_field_error(at, message),
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

/// The built-in integrations that can be added.
#[utoipa::path(get, path = "/api/v1/integrations/catalog", operation_id = "integration_catalog", tag = "integrations",
    responses((status = 200, body = Vec<huxley_integrations::catalog::CatalogEntry>)))]
pub async fn catalog(_authed: Authed) -> Json<Vec<huxley_integrations::catalog::CatalogEntry>> {
    Json(huxley_integrations::catalog::list())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddBuiltInRequest {
    pub org_id: Uuid,
}

/// Adds a built-in integration to an organization, under its catalog key.
#[utoipa::path(post, path = "/api/v1/integrations/catalog/{key}", operation_id = "add_built_in_integration", tag = "integrations",
    params(("key" = String, Path)), request_body = AddBuiltInRequest,
    responses((status = 201, body = IntegrationDetail),
              (status = 404, body = ApiError, content_type = "application/problem+json"),
              (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_built_in(
    State(state): State<AppState>,
    authed: Authed,
    Path(key): Path<String>,
    Json(body): Json<AddBuiltInRequest>,
) -> ApiResult<(StatusCode, Json<IntegrationDetail>)> {
    authed
        .require_at(&state, permissions::INTEGRATION_MANAGE, body.org_id)
        .await?;
    // Built-ins are installed as their pack, so later versions arrive as
    // pack upgrades (with a review).
    let pack = huxley_packs::builtin::for_integration(&key).ok_or_else(ApiError::not_found)?;
    let mut tx = begin_scoped(&state.db, &huxley_db::OrgScope::System)
        .await
        .map_err(db_err)?;
    if crate::packs::existing(&mut tx, body.org_id, "integrations", &key)
        .await
        .map_err(db_err)?
        .is_some()
    {
        return Err(ApiError::conflict(format!(
            "this organization already has an integration called {key:?}"
        )));
    }
    let trust = pack.trust(&[]);
    let items = crate::packs::install(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        &pack,
        body.org_id,
        &trust,
    )
    .await
    .map_err(|e| match e {
        crate::packs::InstallError::Conflict { .. } => ApiError::conflict(e.to_string()),
        other => ApiError::internal(&other),
    })?;
    let id = items["integrations"][&key];
    let detail = store::get(&mut tx, id).await.map_err(map_store)?;
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSpecRequest {
    pub spec: String,
    /// The version the update was previewed against: if the integration
    /// has moved on since, nothing is applied (409).
    pub base_version: Option<i32>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    pub spec: String,
}

/// What importing a newer spec would change, and whether any of it can
/// break workflows or connections. Nothing is saved.
#[utoipa::path(post, path = "/api/v1/integrations/{id}/preview", operation_id = "preview_integration_update", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = PreviewRequest,
    responses((status = 200, body = UpdatePreview), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn preview(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<UpdatePreview>> {
    authorize(&state, &authed, permissions::INTEGRATION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let preview = store::preview(&mut tx, id, &body.spec)
        .await
        .map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(preview))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCurrentRequest {
    pub version: i32,
}

/// Makes an earlier imported version current again (a rollback), or a
/// later one after rolling back.
#[utoipa::path(put, path = "/api/v1/integrations/{id}/current", operation_id = "set_integration_version", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = SetCurrentRequest,
    responses((status = 200, body = IntegrationDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_current(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SetCurrentRequest>,
) -> ApiResult<Json<IntegrationDetail>> {
    authorize(&state, &authed, permissions::INTEGRATION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = store::set_current(&mut tx, &authed.user.ctx, id, body.version)
        .await
        .map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(detail))
}

/// Replaces the integration's overlay: your names, descriptions and groups
/// for its actions, and which are hidden. Kept across new versions.
#[utoipa::path(put, path = "/api/v1/integrations/{id}/overlay", operation_id = "set_integration_overlay", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = huxley_integrations::overlay::Overlay,
    responses((status = 200, body = IntegrationDetail),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_overlay(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<huxley_integrations::overlay::Overlay>,
) -> ApiResult<Json<IntegrationDetail>> {
    authorize(&state, &authed, permissions::INTEGRATION_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let detail = store::set_overlay(&mut tx, &authed.user.ctx, id, body)
        .await
        .map_err(map_store)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(detail))
}

/// Imports a newer spec as the integration's next version, and makes it
/// current. Preview it first to see what changes.
#[utoipa::path(put, path = "/api/v1/integrations/{id}/spec", operation_id = "update_integration_spec", tag = "integrations",
    params(("id" = Uuid, Path)), request_body = UpdateSpecRequest,
    responses((status = 200, body = IntegrationDetail),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
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
    let detail = store::update(&mut tx, &authed.user.ctx, id, &body.spec, body.base_version)
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

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ActionsQuery {
    /// The organization a workflow belongs to.
    pub org_id: Uuid,
}

/// The actions workflows in an organization can call: those of the
/// nearest integration with each key, in the organization or above. For
/// the workflow editor's action picker.
#[utoipa::path(get, path = "/api/v1/integrations/actions", operation_id = "list_available_actions", tag = "integrations",
    params(ActionsQuery), responses((status = 200, body = Vec<huxley_integrations::usage::ActionOption>)))]
pub async fn actions(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ActionsQuery>,
) -> ApiResult<Json<Vec<huxley_integrations::usage::ActionOption>>> {
    authed
        .require_at(&state, permissions::WORKFLOW_READ, query.org_id)
        .await?;
    // Integrations usually live above the workflow's organization.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let options = huxley_integrations::usage::available_actions(&mut tx, query.org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(options))
}
