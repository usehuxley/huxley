//! `/api/v1/scripts`: the script library (design doc §6.9).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_scripts::{NewScript, NewVersion, Script, ScriptError, ScriptSummary, ScriptVersion};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail))
        .route("/{id}/versions", get(versions).post(save_version))
        .route("/{id}/versions/{version}", get(version))
        .route("/{id}/archive", post(archive))
        .route("/{id}/restore", post(restore))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn map_err(e: ScriptError) -> ApiError {
    match e {
        ScriptError::NotFound | ScriptError::VersionNotFound => ApiError::not_found(),
        ScriptError::KeyTaken(_) | ScriptError::Archived => ApiError::conflict(e.to_string()),
        ScriptError::Invalid(field, message) if field == "baseVersion" => {
            ApiError::conflict(message)
        }
        ScriptError::Invalid(field, message) => {
            ApiError::validation().with_field_error(field, message)
        }
        ScriptError::Db(e) => ApiError::internal(&e),
    }
}

async fn authorize(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    id: Uuid,
) -> ApiResult<ScriptSummary> {
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let summary = huxley_scripts::summary(&mut tx, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    authed.require_at(state, permission, summary.org_id).await?;
    Ok(summary)
}

/// Scripts you can see.
#[utoipa::path(get, path = "/api/v1/scripts", operation_id = "list_scripts", tag = "scripts",
    responses((status = 200, body = Vec<ScriptSummary>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<ScriptSummary>>> {
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = huxley_scripts::list(&mut tx).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    let mut paths = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let mut visible = Vec::new();
    for script in all {
        let path: Option<String> =
            sqlx::query_scalar("SELECT path::text FROM organizations WHERE id = $1")
                .bind(script.org_id)
                .fetch_optional(&mut *paths)
                .await
                .map_err(db_err)?;
        if path.is_some_and(|p| authed.principal.can_at_path(permissions::SCRIPT_READ, &p)) {
            visible.push(script);
        }
    }
    paths.commit().await.map_err(db_err)?;
    Ok(Json(visible))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateScriptRequest {
    pub org_id: Uuid,
    pub script: NewScript,
}

#[utoipa::path(post, path = "/api/v1/scripts", operation_id = "create_script", tag = "scripts",
    request_body = CreateScriptRequest,
    responses((status = 201, body = Script), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateScriptRequest>,
) -> ApiResult<(StatusCode, Json<Script>)> {
    authed
        .require_at(&state, permissions::SCRIPT_MANAGE, body.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let script = huxley_scripts::create(&mut tx, &authed.user.ctx, body.org_id, &body.script)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(script)))
}

/// A script with its current version.
#[utoipa::path(get, path = "/api/v1/scripts/{id}", operation_id = "get_script", tag = "scripts",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = Script), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn detail(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Script>> {
    authorize(&state, &authed, permissions::SCRIPT_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let script = huxley_scripts::get(&mut tx, id).await.map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(script))
}

/// Saves the next version and makes it current. With `baseVersion`, a
/// script changed meanwhile is a 409.
#[utoipa::path(post, path = "/api/v1/scripts/{id}/versions", operation_id = "save_script_version", tag = "scripts",
    params(("id" = Uuid, Path)), request_body = NewVersion,
    responses((status = 200, body = Script), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn save_version(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<NewVersion>,
) -> ApiResult<Json<Script>> {
    authorize(&state, &authed, permissions::SCRIPT_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let script = huxley_scripts::save_version(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(script))
}

/// Every version, newest first.
#[utoipa::path(get, path = "/api/v1/scripts/{id}/versions", operation_id = "list_script_versions", tag = "scripts",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vec<ScriptVersion>)))]
pub async fn versions(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<ScriptVersion>>> {
    authorize(&state, &authed, permissions::SCRIPT_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let versions = huxley_scripts::versions(&mut tx, id)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(versions))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VersionSource {
    pub version: i32,
    pub source: String,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub parameters: serde_json::Value,
}

/// One version's source and parameters.
#[utoipa::path(get, path = "/api/v1/scripts/{id}/versions/{version}", operation_id = "get_script_version", tag = "scripts",
    params(("id" = Uuid, Path), ("version" = i32, Path)), responses((status = 200, body = VersionSource)))]
pub async fn version(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, version)): Path<(Uuid, i32)>,
) -> ApiResult<Json<VersionSource>> {
    authorize(&state, &authed, permissions::SCRIPT_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let (source, parameters) = huxley_scripts::version_source(&mut tx, id, version)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(VersionSource {
        version,
        source,
        parameters,
    }))
}

async fn set_archived(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    archived: bool,
) -> ApiResult<Json<Script>> {
    authorize(state, authed, permissions::SCRIPT_MANAGE, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(state).await?)
        .await
        .map_err(db_err)?;
    let script = huxley_scripts::set_archived(&mut tx, &authed.user.ctx, id, archived)
        .await
        .map_err(map_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(script))
}

#[utoipa::path(post, path = "/api/v1/scripts/{id}/archive", operation_id = "archive_script", tag = "scripts",
    params(("id" = Uuid, Path)), responses((status = 200, body = Script)))]
pub async fn archive(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Script>> {
    set_archived(&state, &authed, id, true).await
}

#[utoipa::path(post, path = "/api/v1/scripts/{id}/restore", operation_id = "restore_script", tag = "scripts",
    params(("id" = Uuid, Path)), responses((status = 200, body = Script)))]
pub async fn restore(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Script>> {
    set_archived(&state, &authed, id, false).await
}
