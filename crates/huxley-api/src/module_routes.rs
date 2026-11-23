//! `/api/v1/modules`: feature module toggles, and the gate module routes use.

use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use http::StatusCode;
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_modules::{ModuleError, ModuleState, SetError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{key}", put(set))
}

/// `/orgs/{id}/modules`: per-provider-org narrowing.
pub fn org_routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{id}/modules", get(list_for_org))
        .route("/orgs/{id}/modules/{key}", put(set_for_org))
}

fn map_set(e: SetError) -> ApiError {
    let sentence = |e: &dyn std::fmt::Display| {
        let text = e.to_string();
        format!("{}{}", text[..1].to_uppercase(), &text[1..])
    };
    match e {
        SetError::Module(ModuleError::Unknown(_)) | SetError::OrgNotFound => ApiError::not_found(),
        SetError::Module(
            e @ (ModuleError::MissingDependencies(_) | ModuleError::HasDependents(_)),
        ) => ApiError::new(
            StatusCode::CONFLICT,
            "module-dependencies",
            "Module dependencies",
        )
        .with_detail(sentence(&e)),
        e
        @ (SetError::NotAProviderOrg | SetError::OffInstanceWide | SetError::NarrowedAbove(_)) => {
            ApiError::conflict(format!("{}.", sentence(&e)))
        }
        SetError::Db(e) => db_err(e),
    }
}

/// Rejects requests to a module that isn't active at `org_id` (off for the
/// instance, or narrowed by a provider org above it).
pub async fn require_module_at(state: &AppState, key: &str, org_id: uuid::Uuid) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let active = huxley_modules::is_active_at(&mut tx, key, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    if active {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "module-disabled",
            "Module not enabled",
        )
        .with_detail(format!(
            "The `{key}` module is turned off for this organization."
        )))
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Rejects requests to a feature module that isn't active, as if its routes
/// didn't exist. Every module's handlers call this first.
pub async fn require_module(state: &AppState, key: &str) -> ApiResult<()> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let active = huxley_modules::is_active(&mut tx, key)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    if active {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "module-disabled",
            "Module not enabled",
        )
        .with_detail(format!(
            "The `{key}` module is turned off on this instance."
        )))
    }
}

/// Every feature module and whether it's enabled and active.
#[utoipa::path(get, path = "/api/v1/modules", operation_id = "list_modules", tag = "modules",
    responses((status = 200, body = Vec<ModuleState>)))]
pub async fn list(
    State(state): State<AppState>,
    _authed: Authed,
) -> ApiResult<Json<Vec<ModuleState>>> {
    // Any signed-in user may see which modules exist and run; only admins change them.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let states = huxley_modules::states(&mut tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(states))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetModuleRequest {
    pub enabled: bool,
    /// When enabling, also enable any modules it requires (default false:
    /// respond 409 listing them instead).
    #[serde(default)]
    pub include_dependencies: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetModuleResponse {
    /// Modules whose setting changed, in the order they were applied.
    pub changed: Vec<&'static str>,
    pub modules: Vec<ModuleState>,
}

/// Turns a feature module on or off for the instance. Data is kept when a
/// module is turned off.
#[utoipa::path(put, path = "/api/v1/modules/{key}", operation_id = "set_module", tag = "modules",
    params(("key" = String, Path, description = "Module key, e.g. `servicedesk`")),
    request_body = SetModuleRequest,
    responses(
        (status = 200, body = SetModuleResponse),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 409, description = "Missing dependencies, or enabled modules depend on it",
            body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn set(
    State(state): State<AppState>,
    authed: Authed,
    Path(key): Path<String>,
    Json(body): Json<SetModuleRequest>,
) -> ApiResult<Json<SetModuleResponse>> {
    authed.require_instance(permissions::MODULES_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let changed = huxley_modules::set_enabled(
        &mut tx,
        &authed.user.ctx,
        &key,
        body.enabled,
        body.include_dependencies,
    )
    .await
    .map_err(map_set)?;
    let modules = huxley_modules::states(&mut tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(SetModuleResponse { changed, modules }))
}

/// Every module's state at an organization, including narrowing by a
/// provider org on its chain.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/modules", operation_id = "list_org_modules", tag = "modules",
    params(("id" = uuid::Uuid, Path, description = "Organization id")),
    responses((status = 200, body = Vec<huxley_modules::OrgModuleState>)))]
pub async fn list_for_org(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<uuid::Uuid>,
) -> ApiResult<Json<Vec<huxley_modules::OrgModuleState>>> {
    authed.require_at(&state, permissions::ORG_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let states = huxley_modules::states_at(&mut tx, id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(states))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetOrgModuleRequest {
    /// `false` turns it off here and below; `true` removes that.
    pub enabled: bool,
}

/// Turns a module off for a provider organization (below the root) and its
/// sub-organizations, or back on. Only narrows what the instance has on.
#[utoipa::path(put, path = "/api/v1/orgs/{id}/modules/{key}", operation_id = "set_org_module", tag = "modules",
    params(("id" = uuid::Uuid, Path, description = "Provider organization id"), ("key" = String, Path, description = "Module key")),
    request_body = SetOrgModuleRequest,
    responses((status = 200, body = Vec<huxley_modules::OrgModuleState>),
        (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_for_org(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, key)): Path<(uuid::Uuid, String)>,
    Json(body): Json<SetOrgModuleRequest>,
) -> ApiResult<Json<Vec<huxley_modules::OrgModuleState>>> {
    authed.require_instance(permissions::MODULES_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_modules::set_for_org(&mut tx, &authed.user.ctx, id, &key, body.enabled)
        .await
        .map_err(map_set)?;
    let states = huxley_modules::states_at(&mut tx, id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(states))
}
