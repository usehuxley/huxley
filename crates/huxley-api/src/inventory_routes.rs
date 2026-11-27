//! Inventory (design doc §8.7, ADR-067), under `/inventory`.
//!
//! - `inventory:read` at an organization: its locations, items, stock,
//!   serial numbers, movements and allocations.
//! - `inventory:write`: receive, transfer, adjust, return and RMA stock,
//!   and allocate parts to tickets and projects (which you must be able to
//!   see), then use or release them.
//! - `inventory:configure`: locations, which products are stocked and how
//!   they're tracked, and reorder points.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_inventory::items::{
    self, Filter, Item, ItemInput, Level, ReorderInput, Tracking, Unit, UnitFilter,
};
use huxley_inventory::locations::{self, Location, LocationInput};
use huxley_inventory::stock::{
    self, Allocation, AllocationFilter, AllocationInput, AllocationStatus, Movement,
    MovementFilter, MovementInput,
};
use huxley_inventory::{InventoryError, MODULE};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().nest(
        "/inventory",
        Router::new()
            .route("/locations", get(list_locations).post(create_location))
            .route(
                "/locations/{id}",
                put(update_location).delete(delete_location),
            )
            .route("/items", get(list_items).post(create_item))
            .route(
                "/items/{id}",
                get(get_item).put(update_item).delete(delete_item),
            )
            .route(
                "/items/{id}/reorder/{location}",
                put(set_reorder).delete(clear_reorder),
            )
            .route("/units", get(list_units))
            .route("/movements", get(list_movements).post(create_movement))
            .route(
                "/allocations",
                get(list_allocations).post(create_allocation),
            )
            .route("/allocations/{id}/consume", post(consume_allocation))
            .route("/allocations/{id}/release", post(release_allocation)),
    )
}

fn map(e: InventoryError) -> ApiError {
    match e {
        InventoryError::NotFound => ApiError::not_found(),
        InventoryError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        InventoryError::Conflict(message) => ApiError::conflict(message),
        InventoryError::Db(e) => ApiError::internal(&e),
        InventoryError::Sqlx(e) => ApiError::internal(&e),
    }
}

async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, InventoryError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(value)
}

/// Where a record is (read in system scope), then access there.
async fn access(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    find: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<Option<Uuid>, InventoryError>,
) -> ApiResult<(Uuid, OrgScope)> {
    let org = scoped(state, &OrgScope::System, find)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((org, access_at(state, authed, org, permission).await?))
}

async fn access_at(
    state: &AppState,
    authed: &Authed,
    org: Uuid,
    permission: &str,
) -> ApiResult<OrgScope> {
    require_module_at(state, MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

async fn reading(state: &AppState, authed: &Authed) -> ApiResult<OrgScope> {
    require_module(state, MODULE).await?;
    authed
        .module_scope(state, permissions::INVENTORY_READ, MODULE)
        .await
}

// ---- locations ----

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct LocationParams {
    /// Include locations that are turned off.
    #[serde(default)]
    pub all: bool,
}

/// Locations you can see.
#[utoipa::path(get, path = "/api/v1/inventory/locations", operation_id = "list_inventory_locations",
    tag = "inventory", params(LocationParams), responses((status = 200, body = Vec<Location>)))]
pub async fn list_locations(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<LocationParams>,
) -> ApiResult<Json<Vec<Location>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            locations::list(tx, params.all).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateLocationRequest {
    /// The organization holding the stock (usually the provider).
    pub org_id: Uuid,
    pub location: LocationInput,
}

#[utoipa::path(post, path = "/api/v1/inventory/locations", operation_id = "create_inventory_location",
    tag = "inventory", request_body = CreateLocationRequest,
    responses((status = 201, body = Location), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_location(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateLocationRequest>,
) -> ApiResult<(StatusCode, Json<Location>)> {
    let scope = access_at(
        &state,
        &authed,
        body.org_id,
        permissions::INVENTORY_CONFIGURE,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    let l = scoped(&state, &scope, async |tx| {
        locations::create(tx, &ctx, body.org_id, &body.location).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(l)))
}

#[utoipa::path(put, path = "/api/v1/inventory/locations/{id}", operation_id = "update_inventory_location",
    tag = "inventory", params(("id" = Uuid, Path)), request_body = LocationInput,
    responses((status = 200, body = Location)))]
pub async fn update_location(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<LocationInput>,
) -> ApiResult<Json<Location>> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| locations::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            locations::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Removes a location never used; turn used ones off instead.
#[utoipa::path(delete, path = "/api/v1/inventory/locations/{id}", operation_id = "delete_inventory_location",
    tag = "inventory", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_location(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| locations::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        locations::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- items ----

/// Stocked items you can see, with totals.
#[utoipa::path(get, path = "/api/v1/inventory/items", operation_id = "list_inventory_items", tag = "inventory",
    params(Filter), responses((status = 200, body = Vec<Item>)))]
pub async fn list_items(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<Filter>,
) -> ApiResult<Json<Vec<Item>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| items::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateItemRequest {
    /// The organization stocking it.
    pub org_id: Uuid,
    pub item: ItemInput,
}

/// Starts stocking a catalog product.
#[utoipa::path(post, path = "/api/v1/inventory/items", operation_id = "create_inventory_item", tag = "inventory",
    request_body = CreateItemRequest,
    responses((status = 201, body = Item), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_item(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateItemRequest>,
) -> ApiResult<(StatusCode, Json<Item>)> {
    access_at(
        &state,
        &authed,
        body.org_id,
        permissions::INVENTORY_CONFIGURE,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    // The product may be an ancestor's, out of the caller's sight.
    let item = scoped(&state, &OrgScope::System, async |tx| {
        items::create(tx, &ctx, body.org_id, &body.item).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(item)))
}

/// An item, and where it is.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub item: Item,
    pub levels: Vec<Level>,
}

#[utoipa::path(get, path = "/api/v1/inventory/items/{id}", operation_id = "get_inventory_item", tag = "inventory",
    params(("id" = Uuid, Path)), responses((status = 200, body = ItemDetail)))]
pub async fn get_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ItemDetail>> {
    let (_, scope) = access(&state, &authed, permissions::INVENTORY_READ, async |tx| {
        items::org_of(tx, id).await
    })
    .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            Ok(ItemDetail {
                item: items::get(tx, id).await?,
                levels: items::levels(tx, id).await?,
            })
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateItemRequest {
    pub tracking: Tracking,
}

/// Changes how an item is tracked (while none is on hand), refreshing its
/// SKU and name from the catalog.
#[utoipa::path(put, path = "/api/v1/inventory/items/{id}", operation_id = "update_inventory_item", tag = "inventory",
    params(("id" = Uuid, Path)), request_body = UpdateItemRequest, responses((status = 200, body = Item)))]
pub async fn update_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateItemRequest>,
) -> ApiResult<Json<Item>> {
    access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| items::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            items::update(tx, &ctx, id, body.tracking).await
        })
        .await?,
    ))
}

/// Stops stocking an item that has no history.
#[utoipa::path(delete, path = "/api/v1/inventory/items/{id}", operation_id = "delete_inventory_item",
    tag = "inventory", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_item(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| items::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| items::delete(tx, &ctx, id).await).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets the item's reorder point at a location: `stock.low` is raised when
/// what's available there falls to it.
#[utoipa::path(put, path = "/api/v1/inventory/items/{id}/reorder/{location}", operation_id = "set_inventory_reorder",
    tag = "inventory", params(("id" = Uuid, Path), ("location" = Uuid, Path)), request_body = ReorderInput,
    responses((status = 204)))]
pub async fn set_reorder(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, location)): Path<(Uuid, Uuid)>,
    Json(body): Json<ReorderInput>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| items::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        items::set_reorder(tx, &ctx, id, location, &body).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/inventory/items/{id}/reorder/{location}",
    operation_id = "clear_inventory_reorder", tag = "inventory",
    params(("id" = Uuid, Path), ("location" = Uuid, Path)), responses((status = 204)))]
pub async fn clear_reorder(
    State(state): State<AppState>,
    authed: Authed,
    Path((id, location)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::INVENTORY_CONFIGURE,
        async |tx| items::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        items::clear_reorder(tx, &ctx, id, location).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Serial-numbered units: look one up, or list an item's.
#[utoipa::path(get, path = "/api/v1/inventory/units", operation_id = "list_inventory_units", tag = "inventory",
    params(UnitFilter), responses((status = 200, body = Vec<Unit>)))]
pub async fn list_units(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<UnitFilter>,
) -> ApiResult<Json<Vec<Unit>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| items::units(tx, &filter).await).await?,
    ))
}

// ---- movements ----

/// The ledger, newest first.
#[utoipa::path(get, path = "/api/v1/inventory/movements", operation_id = "list_inventory_movements",
    tag = "inventory", params(MovementFilter), responses((status = 200, body = Vec<Movement>)))]
pub async fn list_movements(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<MovementFilter>,
) -> ApiResult<Json<Vec<Movement>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            stock::movements(tx, &filter).await
        })
        .await?,
    ))
}

/// Receives, transfers, adjusts, returns or sends back (RMA) stock.
#[utoipa::path(post, path = "/api/v1/inventory/movements", operation_id = "create_inventory_movement",
    tag = "inventory", request_body = MovementInput,
    responses((status = 201, body = Movement), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_movement(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<MovementInput>,
) -> ApiResult<(StatusCode, Json<Movement>)> {
    let (_, scope) = access(&state, &authed, permissions::INVENTORY_WRITE, async |tx| {
        items::org_of(tx, body.item_id).await
    })
    .await?;
    let ctx = authed.user.ctx.clone();
    let m = scoped(&state, &scope, async |tx| {
        stock::movement(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(m)))
}

// ---- allocations ----

/// Parts set aside for tickets and projects.
#[utoipa::path(get, path = "/api/v1/inventory/allocations", operation_id = "list_inventory_allocations",
    tag = "inventory", params(AllocationFilter), responses((status = 200, body = Vec<Allocation>)))]
pub async fn list_allocations(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<AllocationFilter>,
) -> ApiResult<Json<Vec<Allocation>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            stock::allocations(tx, &filter).await
        })
        .await?,
    ))
}

/// Sets parts aside for a ticket or project (or, with `consume`, uses them
/// straight away).
#[utoipa::path(post, path = "/api/v1/inventory/allocations", operation_id = "create_inventory_allocation",
    tag = "inventory", request_body = AllocationInput,
    responses((status = 201, body = Allocation), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_allocation(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<AllocationInput>,
) -> ApiResult<(StatusCode, Json<Allocation>)> {
    let (_, scope) = access(&state, &authed, permissions::INVENTORY_WRITE, async |tx| {
        items::org_of(tx, body.item_id).await
    })
    .await?;
    // The ticket or project must be one the caller can see.
    let ref_org = scoped(&state, &authed.scope(&state).await?, async |tx| {
        stock::ref_org(tx, body.ref_type, body.ref_id).await
    })
    .await?
    .ok_or_else(|| {
        ApiError::validation().with_field_error("/refId", "no such ticket or project here")
    })?;
    let ctx = authed.user.ctx.clone();
    let a = scoped(&state, &scope, async |tx| {
        stock::allocate(tx, &ctx, &body, ref_org).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(a)))
}

async fn close(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    to: AllocationStatus,
) -> ApiResult<Allocation> {
    let (_, scope) = access(state, authed, permissions::INVENTORY_WRITE, async |tx| {
        stock::allocation_org(tx, id).await
    })
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(state, &scope, async |tx| {
        stock::close(tx, &ctx, id, to).await
    })
    .await
}

/// The parts were used: they leave stock.
#[utoipa::path(post, path = "/api/v1/inventory/allocations/{id}/consume", operation_id = "consume_inventory_allocation",
    tag = "inventory", params(("id" = Uuid, Path)),
    responses((status = 200, body = Allocation), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn consume_allocation(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Allocation>> {
    Ok(Json(
        close(&state, &authed, id, AllocationStatus::Consumed).await?,
    ))
}

/// The parts weren't needed: they're available again.
#[utoipa::path(post, path = "/api/v1/inventory/allocations/{id}/release", operation_id = "release_inventory_allocation",
    tag = "inventory", params(("id" = Uuid, Path)),
    responses((status = 200, body = Allocation), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn release_allocation(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Allocation>> {
    Ok(Json(
        close(&state, &authed, id, AllocationStatus::Released).await?,
    ))
}
