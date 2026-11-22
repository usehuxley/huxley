//! `/api/v1/orgs`: the organization tree.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::{OrgKind, Organization};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::{Authed, map_org};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(get_one).patch(update))
        .route("/{id}/move", post(move_org))
        .route("/{id}/archive", post(archive))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Organizations the caller can see, ordered as a tree (parents before children).
#[utoipa::path(get, path = "/api/v1/orgs", operation_id = "list_orgs", tag = "orgs",
    responses((status = 200, description = "Visible organizations in tree order", body = Vec<Organization>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Organization>>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        org: Organization,
        path: String,
    }
    let scope = authed.scope(&state).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, parent_id, nlevel(path) AS depth, kind, name, status, version, created_at, \
                updated_at, path::text AS path \
           FROM organizations ORDER BY path",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    // RLS already limits rows to orgs with any grant; keep those with org:read.
    Ok(Json(
        rows.into_iter()
            .filter(|r| authed.principal.can_at_path(permissions::ORG_READ, &r.path))
            .map(|r| r.org)
            .collect(),
    ))
}

#[utoipa::path(get, path = "/api/v1/orgs/{id}", operation_id = "get_org", tag = "orgs",
    params(("id" = Uuid, Path, description = "Organization id")),
    responses(
        (status = 200, body = Organization),
        (status = 404, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn get_one(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Organization>> {
    authed.require_at(&state, permissions::ORG_READ, id).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let org = huxley_orgs::get(&mut tx, id).await.map_err(map_org)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(org))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrgRequest {
    pub parent_id: Uuid,
    pub kind: OrgKindInput,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OrgKindInput {
    Provider,
    Client,
}

impl From<OrgKindInput> for OrgKind {
    fn from(kind: OrgKindInput) -> Self {
        match kind {
            OrgKindInput::Provider => Self::Provider,
            OrgKindInput::Client => Self::Client,
        }
    }
}

/// Creates a sub-organization.
#[utoipa::path(post, path = "/api/v1/orgs", operation_id = "create_org", tag = "orgs",
    request_body = CreateOrgRequest,
    responses(
        (status = 201, body = Organization),
        (status = 409, description = "Name already used by a sibling", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateOrgRequest>,
) -> ApiResult<(StatusCode, Json<Organization>)> {
    authed
        .require_at(&state, permissions::ORG_CREATE, body.parent_id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let org = huxley_orgs::create(
        &mut tx,
        &authed.user.ctx,
        body.parent_id,
        body.kind.into(),
        &body.name,
    )
    .await
    .map_err(map_org)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(org)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOrgRequest {
    pub name: String,
    /// The version you last read; the update fails with 409 if it changed.
    pub version: i32,
}

/// Renames an organization.
#[utoipa::path(patch, path = "/api/v1/orgs/{id}", operation_id = "update_org", tag = "orgs",
    params(("id" = Uuid, Path, description = "Organization id")),
    request_body = UpdateOrgRequest,
    responses((status = 200, body = Organization), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrgRequest>,
) -> ApiResult<Json<Organization>> {
    authed
        .require_at(&state, permissions::ORG_UPDATE, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let org = huxley_orgs::rename(&mut tx, &authed.user.ctx, id, body.version, &body.name)
        .await
        .map_err(map_org)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(org))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveOrgRequest {
    pub parent_id: Uuid,
    pub version: i32,
}

/// Moves an organization and its subtree under a new parent. Requires
/// `org:move` on the organization and `org:create` on the new parent.
#[utoipa::path(post, path = "/api/v1/orgs/{id}/move", operation_id = "move_org", tag = "orgs",
    params(("id" = Uuid, Path, description = "Organization id")),
    request_body = MoveOrgRequest,
    responses((status = 200, body = Organization), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn move_org(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveOrgRequest>,
) -> ApiResult<Json<Organization>> {
    authed.require_at(&state, permissions::ORG_MOVE, id).await?;
    authed
        .require_at(&state, permissions::ORG_CREATE, body.parent_id)
        .await?;
    // Moves rewrite every descendant's path, so they run in system scope.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org = huxley_orgs::move_to(&mut tx, &authed.user.ctx, id, body.version, body.parent_id)
        .await
        .map_err(map_org)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(org))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOrgRequest {
    pub version: i32,
}

/// Archives an organization with no active sub-organizations.
#[utoipa::path(post, path = "/api/v1/orgs/{id}/archive", operation_id = "archive_org", tag = "orgs",
    params(("id" = Uuid, Path, description = "Organization id")),
    request_body = ArchiveOrgRequest,
    responses((status = 200, body = Organization), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ArchiveOrgRequest>,
) -> ApiResult<Json<Organization>> {
    authed
        .require_at(&state, permissions::ORG_ARCHIVE, id)
        .await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let org = huxley_orgs::archive(&mut tx, &authed.user.ctx, id, body.version)
        .await
        .map_err(map_org)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(org))
}
