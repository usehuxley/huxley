//! `/api/v1/packs` (ADR-028): upload a pack, review what it would add to
//! an organization, install or upgrade it, and list what's installed.
//!
//! Installing needs `pack:install` *and* the permissions for what the pack
//! adds (managing integrations, forms, scripts; writing workflows), so a
//! pack never lets anyone do more than they could by hand.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_packs::{Pack, PackError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::packs::{InstallError, Items, Review};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_installed))
        .route("/uploads", post(upload))
        .route("/uploads/{id}", get(review))
        .route("/uploads/{id}/install", post(install))
        .layer(DefaultBodyLimit::max(huxley_packs::MAX_BYTES + 1024))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn pack_err(e: PackError) -> ApiError {
    match e {
        PackError::TooLarge => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "pack-too-large",
            "The pack is too large",
        )
        .with_detail(e.to_string()),
        other => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid-pack",
            "That isn't a valid pack",
        )
        .with_detail(other.to_string()),
    }
}

fn install_err(e: InstallError) -> ApiError {
    match e {
        InstallError::Conflict { .. } => ApiError::conflict(e.to_string()),
        InstallError::Content(message) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid-pack",
            "The pack's contents aren't valid",
        )
        .with_detail(message),
        InstallError::Db(e) => ApiError::internal(&e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct OrgQuery {
    pub org_id: Uuid,
}

/// The permissions needed to install `pack`.
fn needed(pack: &Pack) -> Vec<&'static str> {
    let c = &pack.manifest.contents;
    let mut out = vec![permissions::PACK_INSTALL];
    if !c.integrations.is_empty() {
        out.push(permissions::INTEGRATION_MANAGE);
    }
    if !c.forms.is_empty() {
        out.push(permissions::FORM_MANAGE);
    }
    if !c.scripts.is_empty() {
        out.push(permissions::SCRIPT_MANAGE);
    }
    if !c.workflows.is_empty() || !c.templates.is_empty() {
        out.push(permissions::WORKFLOW_WRITE);
    }
    out
}

/// The caller's upload, read and checked. Uploads belong to whoever made
/// them: anyone else gets a 404.
async fn load(state: &AppState, authed: &Authed, id: Uuid) -> ApiResult<Pack> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let bytes: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT bytes FROM pack_uploads WHERE id = $1 AND uploaded_by = $2 \
           AND created_at > now() - interval '1 day'",
    )
    .bind(id)
    .bind(authed.user.ctx.actor.id())
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Pack::read(&bytes.ok_or_else(ApiError::not_found)?).map_err(pack_err)
}

async fn review_for(state: &AppState, id: Uuid, pack: &Pack, org_id: Uuid) -> ApiResult<Review> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let review = crate::packs::review(&mut tx, id, pack, org_id, &state.trusted_pack_keys)
        .await
        .map_err(install_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(review)
}

/// Uploads a `.hxpack` (the request body) and returns what installing it in
/// `orgId` would do. Nothing is installed yet.
#[utoipa::path(post, path = "/api/v1/packs/uploads", operation_id = "upload_pack", tag = "packs",
    params(OrgQuery), request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 201, body = Review),
              (status = 413, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn upload(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<OrgQuery>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Review>)> {
    authed
        .require_at(&state, permissions::PACK_INSTALL, query.org_id)
        .await?;
    let pack = Pack::read(&body).map_err(pack_err)?;
    let id = huxley_core::new_id();
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    // Old uploads are only for reviewing: clear them out as we go.
    sqlx::query("DELETE FROM pack_uploads WHERE created_at < now() - interval '1 day'")
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    sqlx::query("INSERT INTO pack_uploads (id, uploaded_by, bytes) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(authed.user.ctx.actor.id())
        .bind(body.as_ref())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let review = review_for(&state, id, &pack, query.org_id).await?;
    Ok((StatusCode::CREATED, Json(review)))
}

/// What installing an uploaded pack in `orgId` would do.
#[utoipa::path(get, path = "/api/v1/packs/uploads/{id}", operation_id = "review_pack", tag = "packs",
    params(("id" = Uuid, Path), OrgQuery),
    responses((status = 200, body = Review), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn review(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(query): Query<OrgQuery>,
) -> ApiResult<Json<Review>> {
    authed
        .require_at(&state, permissions::PACK_INSTALL, query.org_id)
        .await?;
    let pack = load(&state, &authed, id).await?;
    Ok(Json(review_for(&state, id, &pack, query.org_id).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRequest {
    pub org_id: Uuid,
    /// The version you reviewed: installing fails if the upload isn't it.
    pub version: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub key: String,
    pub version: String,
    /// What was created or updated, by kind then key.
    #[schema(value_type = std::collections::HashMap<String, std::collections::HashMap<String, Uuid>>)]
    pub items: Items,
}

/// Installs (or upgrades) an uploaded pack in an organization. Workflows
/// arrive as drafts, to review and publish.
#[utoipa::path(post, path = "/api/v1/packs/uploads/{id}/install", operation_id = "install_pack", tag = "packs",
    params(("id" = Uuid, Path)), request_body = InstallRequest,
    responses((status = 201, body = Installed),
              (status = 404, body = ApiError, content_type = "application/problem+json"),
              (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn install(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<InstallRequest>,
) -> ApiResult<(StatusCode, Json<Installed>)> {
    authed
        .require_at(&state, permissions::PACK_INSTALL, body.org_id)
        .await?;
    let pack = load(&state, &authed, id).await?;
    for permission in needed(&pack) {
        authed.require_at(&state, permission, body.org_id).await?;
    }
    if pack.manifest.version != body.version {
        return Err(ApiError::conflict(format!(
            "this upload is version {}, not {}",
            pack.manifest.version, body.version
        )));
    }
    let trust = pack.trust(&state.trusted_pack_keys);
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let items = crate::packs::install(
        &mut tx,
        &authed.user.ctx,
        state.engine.expr(),
        &pack,
        body.org_id,
        &trust,
    )
    .await
    .map_err(install_err)?;
    sqlx::query("DELETE FROM pack_uploads WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(Installed {
            key: pack.manifest.key.clone(),
            version: pack.manifest.version.clone(),
            items,
        }),
    ))
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPack {
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// `trusted`, `unknown_signer` or `unsigned`, when installed.
    pub trust: String,
    #[serde(with = "time::serde::rfc3339")]
    pub installed_at: time::OffsetDateTime,
}

/// The packs installed in an organization.
#[utoipa::path(get, path = "/api/v1/packs", operation_id = "list_installed_packs", tag = "packs",
    params(OrgQuery), responses((status = 200, body = Vec<InstalledPack>)))]
pub async fn list_installed(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<OrgQuery>,
) -> ApiResult<Json<Vec<InstalledPack>>> {
    authed
        .require_at(&state, permissions::INTEGRATION_READ, query.org_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let rows: Vec<InstalledPack> = sqlx::query_as(
        "SELECT pack_key AS key, name, version, publisher, trust, installed_at \
           FROM pack_installs WHERE org_id = $1 ORDER BY name",
    )
    .bind(query.org_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(rows))
}
