//! Sales orders (ADR-069), under `/sales-orders`.
//!
//! - `sales:read` at the client: its orders.
//! - `sales:write`: make one from an accepted quote (accepting makes it
//!   anyway), deliver, close and cancel. Allocating from stock also takes
//!   `inventory:write`, and drafting purchase orders `procurement:write`,
//!   at the client (held, as usual, through a role at the provider).
//! - Handing off: an agreement from the recurring lines (`agreement:manage`)
//!   and a project, from a template if given (`project:manage`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::OrgScope;
use huxley_sales::MODULE;
use huxley_sales::orders::{self, Allocated, Filter, Order, Purchased};
use huxley_sales::quotes::{self, Billing};
use serde::Deserialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::{require_module, require_module_at};
use crate::sales_routes::{org_of, scoped};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().nest(
        "/sales-orders",
        Router::new()
            .route("/", get(list_orders).post(create_order))
            .route("/{id}", get(get_order))
            .route("/{id}/allocate", post(allocate_order))
            .route("/{id}/purchase", post(purchase_order))
            .route("/{id}/fulfil", post(fulfil_order))
            .route("/{id}/agreement", post(order_agreement))
            .route("/{id}/project", post(order_project))
            .route("/{id}/close", post(close_order))
            .route("/{id}/cancel", post(cancel_order)),
    )
}

/// The order's client, with access checked there for each permission.
async fn at_order(
    state: &AppState,
    authed: &Authed,
    id: Uuid,
    perms: &[&str],
) -> ApiResult<(Uuid, OrgScope)> {
    let org = org_of(state, async |tx| orders::org_of(tx, id).await).await?;
    require_module_at(state, MODULE, org).await?;
    for p in perms {
        authed.require_at(state, p, org).await?;
    }
    Ok((org, authed.scope(state).await?))
}

async fn inventory_on(state: &AppState, org: Uuid) -> ApiResult<()> {
    require_module_at(state, huxley_inventory::MODULE, org).await
}

/// Sales orders you can see, newest first.
#[utoipa::path(get, path = "/api/v1/sales-orders", operation_id = "list_sales_orders", tag = "sales",
    params(Filter), responses((status = 200, body = Vec<Order>)))]
pub async fn list_orders(
    State(state): State<AppState>,
    authed: Authed,
    Query(filter): Query<Filter>,
) -> ApiResult<Json<Vec<Order>>> {
    require_module(&state, MODULE).await?;
    let scope = authed
        .module_scope(&state, permissions::SALES_READ, MODULE)
        .await?;
    Ok(Json(
        scoped(&state, &scope, async |tx| orders::list(tx, &filter).await).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSalesOrderRequest {
    /// An accepted quote.
    pub quote_id: Uuid,
}

/// The sales order for an accepted quote (made if it hasn't one; accepting
/// a quote makes it anyway).
#[utoipa::path(post, path = "/api/v1/sales-orders", operation_id = "create_sales_order", tag = "sales",
    request_body = CreateSalesOrderRequest,
    responses((status = 201, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_order(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CreateSalesOrderRequest>,
) -> ApiResult<(StatusCode, Json<Order>)> {
    let org = org_of(&state, async |tx| quotes::org_of(tx, body.quote_id).await).await?;
    require_module_at(&state, MODULE, org).await?;
    authed
        .require_at(&state, permissions::SALES_WRITE, org)
        .await?;
    let scope = authed.scope(&state).await?;
    let ctx = authed.user.ctx.clone();
    let o = scoped(&state, &scope, async |tx| {
        let id = orders::from_quote(tx, &ctx, body.quote_id).await?;
        orders::get(tx, id).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(o)))
}

/// An order with its lines: delivered, set aside and on order.
#[utoipa::path(get, path = "/api/v1/sales-orders/{id}", operation_id = "get_sales_order", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Order)))]
pub async fn get_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Order>> {
    at_order(&state, &authed, id, &[permissions::SALES_READ]).await?;
    // Stock and purchase orders it draws on are the provider's.
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::get(tx, id).await
        })
        .await?,
    ))
}

/// Sets aside what's in stock for the order's one-time product lines.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/allocate", operation_id = "allocate_sales_order", tag = "sales",
    params(("id" = Uuid, Path)), responses((status = 200, body = Allocated)))]
pub async fn allocate_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Allocated>> {
    let (org, _) = at_order(
        &state,
        &authed,
        id,
        &[permissions::SALES_WRITE, permissions::INVENTORY_WRITE],
    )
    .await?;
    inventory_on(&state, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::allocate(tx, &ctx, id).await
        })
        .await?,
    ))
}

/// Drafts purchase orders, per vendor, for what the order is still short.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/purchase", operation_id = "purchase_for_sales_order",
    tag = "sales", params(("id" = Uuid, Path)), responses((status = 200, body = Purchased)))]
pub async fn purchase_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Purchased>> {
    let (org, _) = at_order(
        &state,
        &authed,
        id,
        &[permissions::SALES_WRITE, permissions::PROCUREMENT_WRITE],
    )
    .await?;
    inventory_on(&state, org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::purchase(tx, &ctx, id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SalesOrderVersion {
    pub version: i32,
    /// Why (cancelling), or a closing note.
    #[serde(default)]
    pub note: Option<String>,
}

/// Delivers what's ready: stocked lines use what's set aside for them,
/// services and unstocked lines are delivered in full.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/fulfil", operation_id = "fulfil_sales_order", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SalesOrderVersion, responses((status = 200, body = Order)))]
pub async fn fulfil_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SalesOrderVersion>,
) -> ApiResult<Json<Order>> {
    at_order(&state, &authed, id, &[permissions::SALES_WRITE]).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::fulfil(tx, &ctx, id, body.version).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SalesOrderAgreementRequest {
    /// Default: the order's title.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "managed")]
    pub kind: huxley_contracts::agreements::AgreementKind,
    /// `YYYY-MM-DD`; default today.
    #[serde(default)]
    pub starts_on: Option<String>,
    #[serde(default)]
    pub auto_renew: bool,
    /// Active straight away (default: a draft to finish on the agreement).
    #[serde(default)]
    pub activate: bool,
}

const fn managed() -> huxley_contracts::agreements::AgreementKind {
    huxley_contracts::agreements::AgreementKind::Managed
}

/// Hands the order's recurring lines to a new agreement for the client.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/agreement", operation_id = "sales_order_agreement", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SalesOrderAgreementRequest,
    responses((status = 200, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn order_agreement(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SalesOrderAgreementRequest>,
) -> ApiResult<Json<Order>> {
    let (org, _) = at_order(
        &state,
        &authed,
        id,
        &[permissions::SALES_WRITE, permissions::AGREEMENT_MANAGE],
    )
    .await?;
    require_module_at(&state, huxley_contracts::MODULE, org).await?;
    let starts_on = match body
        .starts_on
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Date::parse(s, time::macros::format_description!("[year]-[month]-[day]"))
            .map_err(|_| {
                ApiError::validation().with_field_error("/startsOn", "a date, YYYY-MM-DD")
            })?,
        None => time::OffsetDateTime::now_utc().date(),
    };
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            let o = orders::get(tx, id).await?;
            let lines: Vec<huxley_contracts::agreements::LineInput> = orders::recurring(&o)
                .into_iter()
                .map(|l| huxley_contracts::agreements::LineInput {
                    product_id: l.product_id,
                    description: Some(l.name.clone()),
                    quantity: l.quantity,
                    unit_price: Some(l.unit_price * (rust_decimal::Decimal::ONE_HUNDRED - l.discount_pct) / rust_decimal::Decimal::ONE_HUNDRED),
                    billing: Some(match l.billing {
                        Billing::Quarterly => huxley_contracts::agreements::LineBilling::Quarterly,
                        Billing::Annual => huxley_contracts::agreements::LineBilling::Annual,
                        _ => huxley_contracts::agreements::LineBilling::Monthly,
                    }),
                })
                .collect();
            if lines.is_empty() {
                return Err(huxley_sales::SalesError::Conflict("the order has no recurring lines".into()));
            }
            let input: huxley_contracts::agreements::AgreementInput = serde_json::from_value(serde_json::json!({
                "name": body.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| o.title.clone()),
                "kind": body.kind,
                "status": if body.activate { "active" } else { "draft" },
                "startsOn": starts_on.to_string(),
                "autoRenew": body.auto_renew,
                "currency": o.currency,
                "notes": format!("From SO-{}.", o.number),
            }))
            .map_err(|e| huxley_sales::SalesError::Conflict(e.to_string()))?;
            let input = huxley_contracts::agreements::AgreementInput { lines, ..input };
            let agreement = huxley_contracts::agreements::create(tx, &ctx, org, &input)
                .await
                .map_err(|e| match e {
                    huxley_contracts::ContractsError::Invalid { field, message } => {
                        huxley_sales::SalesError::Invalid { field, message }
                    }
                    other => huxley_sales::SalesError::Conflict(other.to_string()),
                })?;
            orders::set_agreement(tx, &ctx, id, agreement.id).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SalesOrderProjectRequest {
    /// Default: the order's title.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub template_id: Option<Uuid>,
    /// `YYYY-MM-DD`; default today.
    #[serde(default)]
    pub start_on: Option<String>,
    #[serde(default)]
    pub manager_user_id: Option<Uuid>,
}

/// Starts the order's project for the client, laid out from a template if given.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/project", operation_id = "sales_order_project", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SalesOrderProjectRequest,
    responses((status = 200, body = Order), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn order_project(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SalesOrderProjectRequest>,
) -> ApiResult<Json<Order>> {
    let (org, _) = at_order(
        &state,
        &authed,
        id,
        &[permissions::SALES_WRITE, permissions::PROJECT_MANAGE],
    )
    .await?;
    require_module_at(&state, "projects", org).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            let o = orders::get(tx, id).await?;
            if o.project_id.is_some() {
                return Err(huxley_sales::SalesError::Conflict("it already has a project".into()));
            }
            let input: huxley_servicedesk::projects::NewProject = serde_json::from_value(serde_json::json!({
                "orgId": org,
                "name": body.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| o.title.clone()),
                "description": format!("From SO-{}.", o.number),
                "managerUserId": body.manager_user_id,
                "startOn": body.start_on.clone().filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| time::OffsetDateTime::now_utc().date().to_string()),
                "templateId": body.template_id,
                "agreementId": o.agreement_id,
            }))
            .map_err(|e| huxley_sales::SalesError::Conflict(e.to_string()))?;
            let project = huxley_servicedesk::projects::create(tx, &ctx, &input)
                .await
                .map_err(|e| huxley_sales::SalesError::Conflict(e.to_string()))?;
            orders::set_project(tx, &ctx, id, project.id).await
        })
        .await?,
    ))
}

/// Closes a fulfilled order.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/close", operation_id = "close_sales_order", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SalesOrderVersion, responses((status = 200, body = Order)))]
pub async fn close_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SalesOrderVersion>,
) -> ApiResult<Json<Order>> {
    at_order(&state, &authed, id, &[permissions::SALES_WRITE]).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::finish(tx, &ctx, id, body.version, false, body.note.as_deref()).await
        })
        .await?,
    ))
}

/// Cancels an open order nothing was delivered from, releasing what it set aside.
#[utoipa::path(post, path = "/api/v1/sales-orders/{id}/cancel", operation_id = "cancel_sales_order", tag = "sales",
    params(("id" = Uuid, Path)), request_body = SalesOrderVersion, responses((status = 200, body = Order)))]
pub async fn cancel_order(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SalesOrderVersion>,
) -> ApiResult<Json<Order>> {
    at_order(&state, &authed, id, &[permissions::SALES_WRITE]).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        scoped(&state, &OrgScope::System, async |tx| {
            orders::finish(tx, &ctx, id, body.version, true, body.note.as_deref()).await
        })
        .await?,
    ))
}
