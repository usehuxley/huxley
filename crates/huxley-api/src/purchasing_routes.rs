//! Purchasing (ADR-068): vendors and purchase orders, part of the
//! inventory module.
//!
//! - `procurement:read` at an organization: its vendors and orders.
//! - `procurement:write`: keep vendors, draft, submit, send, cancel and
//!   close orders, and receive deliveries (which also needs nothing more in
//!   inventory: receiving is how stock arrives).
//! - `procurement:approve`: approve or send back orders over the
//!   threshold, and set it.

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_inventory::orders::{
    self, Filter, Order, OrderInput, Receipt, ReceiptInput, Step, StepInput, Suggestion,
};
use huxley_inventory::vendors::{self, Vendor, VendorFilter, VendorInput};
use huxley_inventory::{InventoryError, MODULE};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/vendors", get(list_vendors).post(create_vendor))
        .route(
            "/vendors/{id}",
            get(get_vendor).put(update_vendor).delete(delete_vendor),
        )
        .nest(
            "/purchase-orders",
            Router::new()
                .route("/", get(list_orders).post(create_order))
                .route("/suggestions", get(reorder_suggestions))
                .route("/reorder", post(draft_reorders))
                .route("/settings", get(get_settings).put(put_settings))
                .route(
                    "/{id}",
                    get(get_order).put(update_order).delete(delete_order),
                )
                .route("/{id}/step", post(step_order))
                .route("/{id}/receive", post(receive_order))
                .route("/{id}/receipts", get(order_receipts))
                .route("/{id}/pdf", get(order_pdf)),
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

async fn reading(state: &AppState, authed: &Authed) -> ApiResult<OrgScope> {
    require_module(state, MODULE).await?;
    authed
        .module_scope(state, permissions::PROCUREMENT_READ, MODULE)
        .await
}

// ---- vendors ----

#[utoipa::path(get, path = "/api/v1/vendors", operation_id = "list_vendors", tag = "purchasing",
    params(VendorFilter), responses((status = 200, body = Vec<Vendor>)))]
pub async fn list_vendors(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<VendorFilter>,
) -> ApiResult<Json<Vec<Vendor>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| vendors::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateVendorRequest {
    pub org_id: Uuid,
    pub vendor: VendorInput,
}

#[utoipa::path(post, path = "/api/v1/vendors", operation_id = "create_vendor", tag = "purchasing",
    request_body = CreateVendorRequest,
    responses((status = 201, body = Vendor), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_vendor(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateVendorRequest>,
) -> ApiResult<(StatusCode, Json<Vendor>)> {
    let scope = access_at(&state, &authed, body.org_id, permissions::PROCUREMENT_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let v = scoped(&state, &scope, async |tx| {
        vendors::create(tx, &ctx, body.org_id, &body.vendor).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(v)))
}

#[utoipa::path(get, path = "/api/v1/vendors/{id}", operation_id = "get_vendor", tag = "purchasing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Vendor)))]
pub async fn get_vendor(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vendor>> {
    let (_, scope) = access(&state, &authed, permissions::PROCUREMENT_READ, async |tx| {
        vendors::org_of(tx, id).await
    })
    .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| vendors::get(tx, id).await).await?,
    ))
}

#[utoipa::path(put, path = "/api/v1/vendors/{id}", operation_id = "update_vendor", tag = "purchasing",
    params(("id" = Uuid, Path)), request_body = VendorInput, responses((status = 200, body = Vendor)))]
pub async fn update_vendor(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<VendorInput>,
) -> ApiResult<Json<Vendor>> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::PROCUREMENT_WRITE,
        async |tx| vendors::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            vendors::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Removes a vendor with no orders; turn others off instead.
#[utoipa::path(delete, path = "/api/v1/vendors/{id}", operation_id = "delete_vendor", tag = "purchasing",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_vendor(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::PROCUREMENT_WRITE,
        async |tx| vendors::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        vendors::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- orders ----

/// Purchase orders you can see (without lines), newest first.
#[utoipa::path(get, path = "/api/v1/purchase-orders", operation_id = "list_purchase_orders", tag = "purchasing",
    params(Filter), responses((status = 200, body = Vec<Order>)))]
pub async fn list_orders(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<Filter>,
) -> ApiResult<Json<Vec<Order>>> {
    let scope = reading(&state, &authed).await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| orders::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOrderRequest {
    /// The organization buying (usually the provider).
    pub org_id: Uuid,
    pub order: OrderInput,
}

/// Drafts a purchase order.
#[utoipa::path(post, path = "/api/v1/purchase-orders", operation_id = "create_purchase_order", tag = "purchasing",
    request_body = CreateOrderRequest,
    responses((status = 201, body = Order), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_order(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateOrderRequest>,
) -> ApiResult<(StatusCode, Json<Order>)> {
    let scope = access_at(&state, &authed, body.org_id, permissions::PROCUREMENT_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    let o = scoped(&state, &scope, async |tx| {
        orders::create(tx, &ctx, body.org_id, &body.order).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(o)))
}

#[utoipa::path(get, path = "/api/v1/purchase-orders/{id}", operation_id = "get_purchase_order", tag = "purchasing",
    params(("id" = Uuid, Path)), responses((status = 200, body = Order)))]
pub async fn get_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Order>> {
    let (_, scope) = access(&state, &authed, permissions::PROCUREMENT_READ, async |tx| {
        orders::org_of(tx, id).await
    })
    .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| orders::get(tx, id).await).await?,
    ))
}

/// Changes a draft (send the version you read).
#[utoipa::path(put, path = "/api/v1/purchase-orders/{id}", operation_id = "update_purchase_order", tag = "purchasing",
    params(("id" = Uuid, Path)), request_body = OrderInput,
    responses((status = 200, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<OrderInput>,
) -> ApiResult<Json<Order>> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::PROCUREMENT_WRITE,
        async |tx| orders::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            orders::update(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/purchase-orders/{id}", operation_id = "delete_purchase_order",
    tag = "purchasing", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::PROCUREMENT_WRITE,
        async |tx| orders::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        orders::delete(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes an order a step along: submit, approve, reject, order, cancel or
/// close. Approving and rejecting take `procurement:approve`.
#[utoipa::path(post, path = "/api/v1/purchase-orders/{id}/step", operation_id = "step_purchase_order",
    tag = "purchasing", params(("id" = Uuid, Path)), request_body = StepInput,
    responses((status = 200, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn step_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<StepInput>,
) -> ApiResult<Json<Order>> {
    let permission = match body.step {
        Step::Approve | Step::Reject => permissions::PROCUREMENT_APPROVE,
        _ => permissions::PROCUREMENT_WRITE,
    };
    let (_, scope) = access(&state, &authed, permission, async |tx| {
        orders::org_of(tx, id).await
    })
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            orders::step(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Takes in a delivery, in part or in full.
#[utoipa::path(post, path = "/api/v1/purchase-orders/{id}/receive", operation_id = "receive_purchase_order",
    tag = "purchasing", params(("id" = Uuid, Path)), request_body = ReceiptInput,
    responses((status = 200, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json"),
              (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn receive_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ReceiptInput>,
) -> ApiResult<Json<Order>> {
    let (_, scope) = access(
        &state,
        &authed,
        permissions::PROCUREMENT_WRITE,
        async |tx| orders::org_of(tx, id).await,
    )
    .await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &scope, async |tx| {
            orders::receive(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/purchase-orders/{id}/receipts", operation_id = "purchase_order_receipts",
    tag = "purchasing", params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Receipt>)))]
pub async fn order_receipts(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<Receipt>>> {
    let (_, scope) = access(&state, &authed, permissions::PROCUREMENT_READ, async |tx| {
        orders::org_of(tx, id).await
    })
    .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| orders::receipts(tx, id).await).await?,
    ))
}

/// The order as a PDF, to send to the vendor.
#[utoipa::path(get, path = "/api/v1/purchase-orders/{id}/pdf", operation_id = "purchase_order_pdf",
    tag = "purchasing", params(("id" = Uuid, Path)),
    responses((status = 200, content_type = "application/pdf", body = Vec<u8>)))]
pub async fn order_pdf(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let (_, scope) = access(&state, &authed, permissions::PROCUREMENT_READ, async |tx| {
        orders::org_of(tx, id).await
    })
    .await?;
    let o = scoped(&state, &scope, async |tx| orders::get(tx, id).await).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"PO-{}.pdf\"", o.number),
            ),
        ],
        render_pdf(&o),
    ))
}

fn render_pdf(o: &Order) -> Vec<u8> {
    use huxley_analytics::export::{Cell, Table, pdf};
    let money = |d: Decimal| format!("{} {}", o.currency, d.round_dp(2));
    let rows = o
        .lines
        .iter()
        .map(|l| {
            vec![
                Cell::Text(match &l.sku {
                    Some(sku) => format!("{} ({sku})", l.name),
                    None => l.name.clone(),
                }),
                Cell::Number(l.quantity.normalize().to_string()),
                Cell::Number(money(l.unit_cost)),
                Cell::Number(money(l.total)),
            ]
        })
        .collect();
    let tables = vec![
        Table {
            title: "Order".into(),
            columns: ["Item", "Quantity", "Unit cost", "Total"]
                .map(String::from)
                .to_vec(),
            rows,
            note: None,
        },
        Table {
            title: "Total".into(),
            columns: vec![String::new(), String::new()],
            rows: vec![vec![
                Cell::Text("Total".into()),
                Cell::Number(money(o.total)),
            ]],
            note: o.notes.clone(),
        },
    ];
    let mut sub = vec![o.vendor_name.clone()];
    if let Some(at) = &o.ship_to_name {
        sub.push(format!("deliver to {at}"));
    }
    if let Some(d) = o.expected_on {
        sub.push(format!("expected {d}"));
    }
    pdf(
        &format!("Purchase order PO-{}", o.number),
        &sub.join(" · "),
        &tables,
    )
}

// ---- reordering and settings ----

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct OrgParams {
    /// The buying organization.
    pub org_id: Uuid,
}

/// What's running low at the organization's locations, and what to order.
#[utoipa::path(get, path = "/api/v1/purchase-orders/suggestions", operation_id = "reorder_suggestions",
    tag = "purchasing", params(OrgParams), responses((status = 200, body = Vec<Suggestion>)))]
pub async fn reorder_suggestions(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<OrgParams>,
) -> ApiResult<Json<Vec<Suggestion>>> {
    // Catalog products may be an ancestor's: read in system scope after checking access.
    access_at(
        &state,
        &authed,
        params.org_id,
        permissions::PROCUREMENT_READ,
    )
    .await?;
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::suggestions(tx, params.org_id).await
        })
        .await?,
    ))
}

/// Drafts an order per vendor for what's running low (items whose catalog
/// vendor matches a vendor's name).
#[utoipa::path(post, path = "/api/v1/purchase-orders/reorder", operation_id = "draft_reorders",
    tag = "purchasing", request_body = OrgBody, responses((status = 200, body = Vec<Order>)))]
pub async fn draft_reorders(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<OrgBody>,
) -> ApiResult<Json<Vec<Order>>> {
    access_at(&state, &authed, body.org_id, permissions::PROCUREMENT_WRITE).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::draft_reorders(tx, &ctx, body.org_id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgBody {
    pub org_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PurchasingSettings {
    pub org_id: Uuid,
    /// Orders over this total need approval; none: never.
    #[schema(value_type = Option<String>)]
    pub approval_above: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/purchase-orders/settings", operation_id = "get_purchasing_settings",
    tag = "purchasing", params(OrgParams), responses((status = 200, body = PurchasingSettings)))]
pub async fn get_settings(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<OrgParams>,
) -> ApiResult<Json<PurchasingSettings>> {
    let scope = access_at(
        &state,
        &authed,
        params.org_id,
        permissions::PROCUREMENT_READ,
    )
    .await?;
    let above = scoped(&state, &scope, async |tx| {
        orders::approval_above(tx, params.org_id).await
    })
    .await?;
    Ok(Json(PurchasingSettings {
        org_id: params.org_id,
        approval_above: above.map(|a| a.normalize().to_string()),
    }))
}

#[utoipa::path(put, path = "/api/v1/purchase-orders/settings", operation_id = "put_purchasing_settings",
    tag = "purchasing", request_body = PurchasingSettings, responses((status = 200, body = PurchasingSettings)))]
pub async fn put_settings(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<PurchasingSettings>,
) -> ApiResult<Json<PurchasingSettings>> {
    let scope = access_at(
        &state,
        &authed,
        body.org_id,
        permissions::PROCUREMENT_APPROVE,
    )
    .await?;
    let above = match body
        .approval_above
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Some(s.parse::<Decimal>().map_err(|_| {
            ApiError::validation().with_field_error("/approvalAbove", "isn't a number")
        })?),
        None => None,
    };
    let ctx = authed.user.ctx.clone();
    scoped(&state, &scope, async |tx| {
        orders::set_approval_above(tx, &ctx, body.org_id, above).await
    })
    .await?;
    Ok(Json(PurchasingSettings {
        org_id: body.org_id,
        approval_above: above.map(|a| a.normalize().to_string()),
    }))
}
