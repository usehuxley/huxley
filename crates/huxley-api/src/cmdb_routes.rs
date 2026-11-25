//! The `cmdb` module's API (design doc §8.5): assets, their types and
//! relationships, and impact analysis.
//!
//! - `asset:read` at an organization: its assets, relationships and impact.
//! - `asset:write`: add, change, relate and delete them.
//! - `cmdb:configure` (instance-wide): keep asset types and their fields.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_cmdb::assets::{self, Asset, AssetInput, Filter};
use huxley_cmdb::relationships::{self, DependencyMap, Impacted, Relationship, RelationshipInput};
use huxley_cmdb::tickets::{self, AssetTicket, TicketAsset};
use huxley_cmdb::types::{self, AssetType, AssetTypeInput};
use huxley_cmdb::{CmdbError, MODULE};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/asset-types",
            get(list_asset_types).post(create_asset_type),
        )
        .route(
            "/asset-types/{id}",
            put(update_asset_type).delete(archive_asset_type),
        )
        .route("/assets", get(list_assets).post(create_asset))
        .route(
            "/assets/{id}",
            get(get_asset).put(update_asset).delete(delete_asset),
        )
        .route("/assets/{id}/relationships", post(relate_asset))
        .route("/assets/{id}/impact", get(asset_impact))
        .route("/asset-relationships/{id}", delete(unrelate_asset))
        .route("/assets/{id}/map", get(asset_map))
        .route("/assets/{id}/tickets", get(asset_tickets))
        .route(
            "/tickets/{ticket_id}/assets",
            get(ticket_assets).post(link_ticket_asset),
        )
        .route(
            "/tickets/{ticket_id}/assets/{asset_id}",
            delete(unlink_ticket_asset),
        )
}

fn map(e: CmdbError) -> ApiError {
    match e {
        CmdbError::NotFound => ApiError::not_found(),
        CmdbError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        CmdbError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        CmdbError::Db(e) => ApiError::internal(&e),
        CmdbError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, CmdbError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db_err)?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(value)
}

/// The organization a record belongs to (404 if there's none).
async fn owner(state: &AppState, table: &str, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT org_id FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

/// The module is on at `org_id` and the caller holds `permission` there:
/// the scope to work in.
async fn allow(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org_id: Uuid,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org_id).await?;
    authed.require_at(state, permission, org_id).await?;
    authed.scope(state).await
}

async fn asset_access(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    let org = owner(state, "assets", id).await?;
    allow(state, authed, permission, org).await
}

/// Whether the caller holds `permission` at `org_id`.
async fn can_at(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org_id: Uuid,
) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let path = huxley_authz::org_path(&mut tx, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(authed.principal.can_at_path(permission, &path))
}

// ---- Types ----

async fn configurer(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    authed.require_instance(permissions::CMDB_CONFIGURE)
}

/// Asset types and their fields (anyone who may read assets somewhere).
#[utoipa::path(get, path = "/api/v1/asset-types", tag = "cmdb",
    responses((status = 200, body = Vec<AssetType>)))]
pub async fn list_asset_types(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<AssetType>>> {
    require_module(&state, MODULE).await?;
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::ASSET_READ)
    {
        return Err(ApiError::forbidden());
    }
    Ok(Json(scoped(&state, &OrgScope::System, types::list).await?))
}

/// Adds an asset type (`cmdb:configure`).
#[utoipa::path(post, path = "/api/v1/asset-types", tag = "cmdb", request_body = AssetTypeInput,
    responses((status = 201, body = AssetType), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_asset_type(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<AssetTypeInput>,
) -> ApiResult<(StatusCode, Json<AssetType>)> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let t = scoped(&state, &OrgScope::System, async |tx| {
        types::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(t)))
}

/// Changes an asset type's name, fields and position (`cmdb:configure`);
/// send back the `version` you read.
#[utoipa::path(put, path = "/api/v1/asset-types/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    request_body = AssetTypeInput,
    responses((status = 200, body = AssetType), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_asset_type(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<AssetTypeInput>,
) -> ApiResult<Json<AssetType>> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            types::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a custom asset type: no new assets of it (`cmdb:configure`).
#[utoipa::path(delete, path = "/api/v1/asset-types/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_asset_type(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    configurer(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &OrgScope::System, async |tx| {
        types::archive(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Assets ----

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AssetParams {
    /// Words in the name, serial number, hostname, model or manufacturer.
    pub q: Option<String>,
    pub org_id: Option<Uuid>,
    pub type_id: Option<Uuid>,
    pub status: Option<String>,
    /// Leave out retired and disposed assets.
    #[serde(default)]
    pub in_use: bool,
    /// Default 100, at most 500.
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AssetPage {
    pub items: Vec<Asset>,
    pub total: i64,
}

/// Assets you can see, by name.
#[utoipa::path(get, path = "/api/v1/assets", tag = "cmdb", params(AssetParams),
    responses((status = 200, body = AssetPage)))]
pub async fn list_assets(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<AssetParams>,
) -> ApiResult<Json<AssetPage>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::ASSET_READ, MODULE)
        .await?;
    let filter = Filter {
        q: params.q,
        org_id: params.org_id,
        type_id: params.type_id,
        status: params.status,
        in_use: params.in_use,
        limit: params.limit.unwrap_or(100),
        offset: params.offset.unwrap_or(0),
    };
    let (items, total) = scoped(&state, &scope, async |tx| assets::list(tx, &filter).await).await?;
    Ok(Json(AssetPage { items, total }))
}

/// An asset with its relationships, and whether you may change it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetView {
    #[serde(flatten)]
    pub asset: Asset,
    pub relationships: Vec<Relationship>,
    pub can_write: bool,
}

async fn view(
    state: &AppState,
    authed: &Authed,
    scope: &OrgScope,
    asset: Asset,
) -> ApiResult<AssetView> {
    let id = asset.id;
    let relationships = scoped(state, scope, async |tx| {
        relationships::for_asset(tx, id).await
    })
    .await?;
    let can_write = can_at(state, authed, permissions::ASSET_WRITE, asset.org_id).await?;
    Ok(AssetView {
        asset,
        relationships,
        can_write,
    })
}

/// Adds an asset at an organization (`asset:write` there).
#[utoipa::path(post, path = "/api/v1/assets", tag = "cmdb", request_body = AssetInput,
    responses((status = 201, body = AssetView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_asset(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<AssetInput>,
) -> ApiResult<(StatusCode, Json<AssetView>)> {
    let org = body.org_id.ok_or_else(|| {
        ApiError::validation().with_field_error("/orgId", "which organization it belongs to")
    })?;
    let scope = allow(&state, &authed, permissions::ASSET_WRITE, org).await?;
    let ctx = authed.user.ctx.clone();
    let asset = scoped(&state, &scope, async |tx| {
        assets::create(tx, &ctx, &body).await
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state, &authed, &scope, asset).await?),
    ))
}

/// An asset, with its relationships.
#[utoipa::path(get, path = "/api/v1/assets/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    responses((status = 200, body = AssetView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<AssetView>> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_READ).await?;
    let asset = scoped(&state, &scope, async |tx| assets::get(tx, id).await).await?;
    Ok(Json(view(&state, &authed, &scope, asset).await?))
}

/// Replaces an asset's fields (send them all, with the `version` you read).
#[utoipa::path(put, path = "/api/v1/assets/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    request_body = AssetInput,
    responses((status = 200, body = AssetView), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<AssetInput>,
) -> ApiResult<Json<AssetView>> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let asset = scoped(&state, &scope, async |tx| {
        assets::update(tx, &ctx, id, &body).await
    })
    .await?;
    Ok(Json(view(&state, &authed, &scope, asset).await?))
}

/// Deletes an asset and its relationships (for mistakes; retire it to
/// keep its history).
#[utoipa::path(delete, path = "/api/v1/assets/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    responses((status = 204)))]
pub async fn delete_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        assets::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Relates this asset to another: this one `kind` it (`asset:write` here,
/// `asset:read` where the other is). Relating them again is fine.
#[utoipa::path(post, path = "/api/v1/assets/{id}/relationships", tag = "cmdb", params(("id" = Uuid, Path)),
    request_body = RelationshipInput,
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn relate_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<RelationshipInput>,
) -> ApiResult<StatusCode> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_WRITE).await?;
    asset_access(&state, &authed, body.to_id, permissions::ASSET_READ).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        relationships::add(tx, &ctx, id, &body).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a relationship (`asset:write` where it belongs).
#[utoipa::path(delete, path = "/api/v1/asset-relationships/{id}", tag = "cmdb", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn unrelate_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = owner(&state, "cmdb_relationships", id).await?;
    let scope = allow(&state, &authed, permissions::ASSET_WRITE, org).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        relationships::remove(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ImpactParams {
    /// How many steps to follow (default 10, at most 20).
    pub depth: Option<i32>,
}

/// What breaks if this asset goes down: what (transitively) runs on it,
/// depends on it, is licensed by it, or is hosted by it, nearest first.
#[utoipa::path(get, path = "/api/v1/assets/{id}/impact", tag = "cmdb", params(("id" = Uuid, Path), ImpactParams),
    responses((status = 200, body = Vec<Impacted>)))]
pub async fn asset_impact(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<ImpactParams>,
) -> ApiResult<Json<Vec<Impacted>>> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_READ).await?;
    let depth = params.depth.unwrap_or(10);
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            relationships::impact(tx, id, depth).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct MapParams {
    /// How many relationships out to show (default 2, at most 4).
    pub depth: Option<i32>,
}

/// The assets around this one, either way, and the relationships between
/// them: its dependency map (at most 150 assets).
#[utoipa::path(get, path = "/api/v1/assets/{id}/map", tag = "cmdb", params(("id" = Uuid, Path), MapParams),
    responses((status = 200, body = DependencyMap)))]
pub async fn asset_map(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Query(params): Query<MapParams>,
) -> ApiResult<Json<DependencyMap>> {
    let scope = asset_access(&state, &authed, id, permissions::ASSET_READ).await?;
    let depth = params.depth.unwrap_or(2);
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            relationships::map(tx, id, depth).await
        })
        .await?,
    ))
}

/// Whether the service desk is on, and the caller may read tickets.
async fn desk_readable(state: &AppState, authed: &Authed) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let on = huxley_modules::is_active(&mut tx, huxley_servicedesk::MODULE)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(on
        && authed
            .principal
            .granted_anywhere()
            .contains(&permissions::TICKET_READ))
}

/// The tickets an asset is linked to that you can see, newest first.
#[utoipa::path(get, path = "/api/v1/assets/{id}/tickets", tag = "cmdb", params(("id" = Uuid, Path)),
    responses((status = 200, body = Vec<AssetTicket>)))]
pub async fn asset_tickets(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<AssetTicket>>> {
    asset_access(&state, &authed, id, permissions::ASSET_READ).await?;
    if !desk_readable(&state, &authed).await? {
        return Ok(Json(Vec::new()));
    }
    let scope = authed
        .module_scope(&state, permissions::TICKET_READ, huxley_servicedesk::MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| tickets::for_asset(tx, id).await).await?,
    ))
}

/// A ticket's organization, checked: assets and the service desk are on
/// there, and the caller holds `permission` on the ticket.
async fn ticket_org(
    state: &AppState,
    authed: &Authed,
    ticket: Uuid,
    permission: &str,
) -> ApiResult<Uuid> {
    require_module(state, MODULE).await?;
    let org = owner(state, "sd_tickets", ticket).await?;
    require_module_at(state, huxley_servicedesk::MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    Ok(org)
}

/// The assets on a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketAssets {
    /// Assets linked to it.
    pub linked: Vec<TicketAsset>,
    /// The requester's assets in use, not yet linked: the likely ones.
    pub requester_assets: Vec<TicketAsset>,
    /// You may link and unlink assets (`ticket:write` on the ticket).
    pub can_link: bool,
}

/// The assets linked to a ticket, and its requester's other assets
/// (`ticket:read` on the ticket; what you may read of assets).
#[utoipa::path(get, path = "/api/v1/tickets/{ticket_id}/assets", tag = "cmdb", params(("ticket_id" = Uuid, Path)),
    responses((status = 200, body = TicketAssets), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn ticket_assets(
    State(state): State<AppState>,
    authed: Authed,
    Path(ticket_id): Path<Uuid>,
) -> ApiResult<Json<TicketAssets>> {
    let org = ticket_org(&state, &authed, ticket_id, permissions::TICKET_READ).await?;
    let can_link = can_at(&state, &authed, permissions::TICKET_WRITE, org).await?;
    if !authed
        .principal
        .granted_anywhere()
        .contains(&permissions::ASSET_READ)
    {
        return Ok(Json(TicketAssets {
            linked: Vec::new(),
            requester_assets: Vec::new(),
            can_link: false,
        }));
    }
    let scope = authed
        .module_scope(&state, permissions::ASSET_READ, MODULE)
        .await?;
    let (linked, requester_assets) = scoped(&state, &scope, async |tx| {
        Ok((
            tickets::for_ticket(tx, ticket_id).await?,
            tickets::requester_assets(tx, ticket_id).await?,
        ))
    })
    .await?;
    Ok(Json(TicketAssets {
        linked,
        requester_assets,
        can_link,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkTicketAsset {
    pub asset_id: Uuid,
}

/// Links an asset to a ticket (`ticket:write` on the ticket, `asset:read`
/// where the asset is). It must be the ticket's client's, or the MSP's.
#[utoipa::path(post, path = "/api/v1/tickets/{ticket_id}/assets", tag = "cmdb", params(("ticket_id" = Uuid, Path)),
    request_body = LinkTicketAsset,
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn link_ticket_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path(ticket_id): Path<Uuid>,
    Json(body): Json<LinkTicketAsset>,
) -> ApiResult<StatusCode> {
    ticket_org(&state, &authed, ticket_id, permissions::TICKET_WRITE).await?;
    asset_access(&state, &authed, body.asset_id, permissions::ASSET_READ).await?;
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        tickets::link(tx, &ctx, body.asset_id, ticket_id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes an asset from a ticket (`ticket:write` on the ticket).
#[utoipa::path(delete, path = "/api/v1/tickets/{ticket_id}/assets/{asset_id}", tag = "cmdb",
    params(("ticket_id" = Uuid, Path), ("asset_id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn unlink_ticket_asset(
    State(state): State<AppState>,
    authed: Authed,
    Path((ticket_id, asset_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    ticket_org(&state, &authed, ticket_id, permissions::TICKET_WRITE).await?;
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        tickets::unlink(tx, &ctx, asset_id, ticket_id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
