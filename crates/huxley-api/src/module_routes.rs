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
    .map_err(|e| match e {
        SetError::Module(ModuleError::Unknown(_)) => ApiError::not_found(),
        SetError::Module(
            e @ (ModuleError::MissingDependencies(_) | ModuleError::HasDependents(_)),
        ) => ApiError::new(
            StatusCode::CONFLICT,
            "module-dependencies",
            "Module dependencies",
        )
        .with_detail(format!(
            "{}{}",
            e.to_string()[..1].to_uppercase(),
            &e.to_string()[1..]
        )),
        SetError::Db(e) => db_err(e),
    })?;
    let modules = huxley_modules::states(&mut tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(SetModuleResponse { changed, modules }))
}
