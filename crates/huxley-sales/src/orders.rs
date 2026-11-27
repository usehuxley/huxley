//! Sales orders (ADR-069): what an accepted quote turns into.
//!
//! An order is made when a quote is accepted, at the client, with the
//! lines agreed to (options taken, not the rest). It's then fulfilled:
//!
//! - **from stock**: one-time product lines are allocated from the nearest
//!   organization above the client that stocks them, then used when
//!   delivered;
//! - **by purchase**: what's short is drafted onto purchase orders per
//!   vendor, bought for the order, and allocated to it on arrival;
//! - **by hand-off**: recurring lines go to an agreement, and the work to a
//!   project (the API makes those, then records them here).
//!
//! Lines with no stock to draw on (services, unstocked products) are
//! fulfilled when the order is. Once every line is, it's fulfilled and
//! `sales_order.fulfilled` is raised (for billing).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_inventory::items::Tracking;
use huxley_inventory::stock::{self, AllocationInput, AllocationStatus, RefType};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::quotes::Billing;
use crate::{SalesError, SalesResult, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = SalesOrderStatus)]
pub enum Status {
    Open,
    Fulfilled,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesOrderLine)]
pub struct Line {
    pub id: Uuid,
    pub product_id: Option<Uuid>,
    pub sku: Option<String>,
    pub name: String,
    pub description: String,
    pub billing: Billing,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub unit: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub discount_pct: Decimal,
    /// What was quoted as its cost (what it's bought at, by default).
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub unit_cost: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub fulfilled: Decimal,
    /// The stocked item it's drawn from (the nearest organization above the
    /// client that stocks the product), if any.
    pub item_id: Option<Uuid>,
    pub tracking: Option<Tracking>,
    /// Set aside for this order (per item).
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub allocated: Decimal,
    /// On purchase orders bought for this order, still to arrive (per item).
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub on_order: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesOrder)]
pub struct Order {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub number: i64,
    pub quote_id: Option<Uuid>,
    pub quote_number: Option<i64>,
    pub opportunity_id: Option<Uuid>,
    pub title: String,
    pub currency: String,
    pub status: Status,
    pub agreement_id: Option<Uuid>,
    pub agreement_name: Option<String>,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub notes: Option<String>,
    pub created_by_name: Option<String>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub one_time: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub monthly: Decimal,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    #[sqlx(skip)]
    pub lines: Vec<Line>,
}

const TOTAL: &str = "round(l.quantity * l.unit_price * (100 - l.discount_pct) / 100, 2)";

fn select() -> String {
    format!(
        "SELECT s.id, s.org_id, o.name AS org_name, s.number, s.quote_id, q.number AS quote_number, \
            s.opportunity_id, s.title, s.currency, s.status, s.agreement_id, a.name AS agreement_name, \
            s.project_id, p.name AS project_name, s.notes, u.display_name AS created_by_name, \
            coalesce((SELECT sum({TOTAL}) FROM sales_order_lines l WHERE l.order_id = s.id \
                       AND l.billing = 'one_time'), 0) AS one_time, \
            coalesce((SELECT sum({TOTAL} / CASE l.billing WHEN 'monthly' THEN 1 WHEN 'quarterly' THEN 3 ELSE 12 END) \
                        FROM sales_order_lines l WHERE l.order_id = s.id AND l.billing <> 'one_time'), 0) AS monthly, \
            s.version, s.created_at, s.updated_at, s.closed_at \
       FROM sales_orders s JOIN organizations o ON o.id = s.org_id \
       LEFT JOIN sales_quotes q ON q.id = s.quote_id \
       LEFT JOIN contract_agreements a ON a.id = s.agreement_id \
       LEFT JOIN prj_projects p ON p.id = s.project_id \
       LEFT JOIN users u ON u.id = s.created_by"
    )
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub status: Option<Status>,
    pub org_id: Option<Uuid>,
    /// Title, client, or `SO-` number.
    pub q: Option<String>,
}

pub async fn list(conn: &mut PgConnection, filter: &Filter) -> SalesResult<Vec<Order>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let number: Option<i64> = q.and_then(|q| {
        q.to_ascii_uppercase()
            .trim_start_matches("SO-")
            .parse()
            .ok()
    });
    let like = q.map(|q| {
        format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let status = filter.status.map(|s| match s {
        Status::Open => "open",
        Status::Fulfilled => "fulfilled",
        Status::Closed => "closed",
        Status::Cancelled => "cancelled",
    });
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{} WHERE ($1::text IS NULL OR s.status = $1) AND ($2::uuid IS NULL OR s.org_id = $2) \
            AND ($3::text IS NULL OR s.title ILIKE $3 OR o.name ILIKE $3 OR s.number = $4) \
          ORDER BY s.created_at DESC LIMIT 500",
        select()
    )))
    .bind(status)
    .bind(filter.org_id)
    .bind(like)
    .bind(number)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Order> {
    let mut o: Order = sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE s.id = $1", select())))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(SalesError::NotFound)?;
    o.lines = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT l.id, l.product_id, l.sku, l.name, l.description, l.billing, l.quantity, l.unit, l.unit_price, \
                l.discount_pct, l.unit_cost, {TOTAL} AS total, l.fulfilled, it.id AS item_id, it.tracking, \
                coalesce((SELECT sum(a.quantity) FROM inventory_allocations a WHERE a.ref_type = 'sales_order' \
                           AND a.ref_id = l.order_id AND a.item_id = it.id AND a.status = 'allocated'), 0) AS allocated, \
                coalesce((SELECT sum(x.quantity - x.received) FROM procurement_order_lines x \
                            JOIN procurement_orders po ON po.id = x.order_id \
                           WHERE x.for_ref_type = 'sales_order' AND x.for_ref_id = l.order_id AND x.item_id = it.id \
                             AND po.status NOT IN ('received', 'cancelled')), 0) AS on_order \
           FROM sales_order_lines l \
           LEFT JOIN LATERAL ( \
               SELECT i.id, i.tracking FROM inventory_items i JOIN organizations io ON io.id = i.org_id \
                 JOIN organizations c ON c.id = l.org_id \
                WHERE i.product_id = l.product_id AND io.path @> c.path \
                ORDER BY nlevel(io.path) DESC LIMIT 1) it ON l.billing = 'one_time' \
          WHERE l.order_id = $1 ORDER BY l.position"
    )))
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(o)
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_orders WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// The sales order of a quote, if it has one.
pub async fn of_quote(conn: &mut PgConnection, quote: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM sales_orders WHERE quote_id = $1")
            .bind(quote)
            .fetch_optional(conn)
            .await?,
    )
}

/// Makes the sales order for an accepted quote (or returns the one it has).
pub async fn from_quote(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    quote: Uuid,
) -> SalesResult<Uuid> {
    if let Some(id) = of_quote(&mut *conn, quote).await? {
        return Ok(id);
    }
    let q = crate::quotes::get(&mut *conn, quote).await?;
    if q.status != crate::quotes::Status::Accepted {
        return Err(SalesError::Conflict(
            "only an accepted quote becomes an order".into(),
        ));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_orders (id, org_id, quote_id, opportunity_id, title, currency, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(q.org_id)
    .bind(q.id)
    .bind(q.opportunity_id)
    .bind(&q.title)
    .bind(&q.currency)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO sales_order_lines (id, order_id, org_id, position, product_id, sku, name, description, billing, \
            quantity, unit, unit_price, discount_pct, unit_cost) \
         SELECT gen_random_uuid(), $1, $2, l.position, l.product_id, l.sku, l.name, l.description, l.billing, \
                l.quantity, l.unit, l.unit_price, l.discount_pct, l.unit_cost \
           FROM sales_quote_lines l \
          WHERE l.quote_id = $3 AND l.kind = 'item' AND (NOT l.optional OR l.selected) AND l.quantity > 0",
    )
    .bind(id)
    .bind(q.org_id)
    .bind(q.id)
    .execute(&mut *conn)
    .await?;
    let o = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_order.created")
            .org(o.org_id)
            .target("sales_order", id)
            .details(json!({ "number": o.number, "quote": q.number })),
    )
    .await?;
    emit(&mut *conn, "sales_order.created", &o).await?;
    Ok(id)
}

async fn emit(conn: &mut PgConnection, event: &'static str, o: &Order) -> SalesResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(o.org_id),
            subject_type: "sales_order",
            subject_id: o.id,
            payload: json!({
                "salesOrderId": o.id, "reference": format!("SO-{}", o.number), "title": o.title,
                "orgId": o.org_id, "client": o.org_name, "quoteId": o.quote_id, "status": o.status,
                "currency": o.currency, "oneTime": o.one_time.normalize().to_string(),
                "monthly": o.monthly.normalize().to_string(), "agreementId": o.agreement_id,
                "projectId": o.project_id, "link": format!("/sales/orders/{}", o.id),
            }),
        },
    )
    .await?;
    Ok(())
}

/// Locks an open order (checking the version read, if given).
async fn open(conn: &mut PgConnection, id: Uuid, version: Option<i32>) -> SalesResult<Order> {
    let (status, current): (Status, i32) =
        sqlx::query_as("SELECT status, version FROM sales_orders WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(SalesError::NotFound)?;
    if version.is_some_and(|v| v != current) {
        return Err(SalesError::VersionConflict);
    }
    if status != Status::Open {
        return Err(SalesError::Conflict("the order isn't open".into()));
    }
    get(conn, id).await
}

async fn touch(conn: &mut PgConnection, id: Uuid) -> SalesResult<()> {
    sqlx::query("UPDATE sales_orders SET version = version + 1, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// What a line still needs from stock: not yet delivered, allocated or on order.
fn short(l: &Line) -> Decimal {
    (l.quantity - l.fulfilled - l.allocated - l.on_order).max(Decimal::ZERO)
}

// ---- from stock ----

/// What an allocation pass did.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesOrderAllocated)]
pub struct Allocated {
    /// Allocations made.
    pub made: usize,
    /// Lines still short afterwards: `name: how many`.
    pub short: Vec<String>,
}

/// Sets aside what's available for the order's one-time stocked lines,
/// from the locations with the most first.
pub async fn allocate(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> SalesResult<Allocated> {
    let o = open(&mut *conn, id, None).await?;
    let mut out = Allocated::default();
    for l in &o.lines {
        let (Some(item), Some(tracking)) = (l.item_id, l.tracking) else {
            continue;
        };
        let mut need = (l.quantity - l.fulfilled - l.allocated).max(Decimal::ZERO);
        if need.is_zero() {
            continue;
        }
        let rows: Vec<(Uuid, String, Decimal)> = sqlx::query_as(
            "SELECT location_id, lot, on_hand - allocated FROM inventory_stock \
              WHERE item_id = $1 AND on_hand > allocated ORDER BY on_hand - allocated DESC",
        )
        .bind(item)
        .fetch_all(&mut *conn)
        .await?;
        for (location, lot, available) in rows {
            if need.is_zero() {
                break;
            }
            let take = if tracking == Tracking::Serial {
                need.floor().min(available.floor())
            } else {
                need.min(available)
            };
            if take <= Decimal::ZERO {
                continue;
            }
            let serials: Vec<String> = if tracking == Tracking::Serial {
                sqlx::query_scalar(
                    "SELECT serial FROM inventory_units WHERE item_id = $1 AND location_id = $2 \
                       AND status = 'in_stock' ORDER BY serial LIMIT $3",
                )
                .bind(item)
                .bind(location)
                .bind(i64::try_from(take.trunc().mantissa()).unwrap_or(0))
                .fetch_all(&mut *conn)
                .await?
            } else {
                Vec::new()
            };
            let take = if tracking == Tracking::Serial {
                Decimal::from(serials.len())
            } else {
                take
            };
            if take.is_zero() {
                continue;
            }
            stock::allocate(
                &mut *conn,
                ctx,
                &AllocationInput {
                    item_id: item,
                    location_id: location,
                    quantity: (tracking != Tracking::Serial).then_some(take),
                    lot: (!lot.is_empty()).then(|| lot.clone()),
                    serials,
                    ref_type: RefType::SalesOrder,
                    ref_id: o.id,
                    note: Some(format!("SO-{}", o.number)),
                    consume: false,
                },
                o.org_id,
            )
            .await
            .map_err(inventory)?;
            need -= take;
            out.made += 1;
        }
        let left = (need - l.on_order).max(Decimal::ZERO);
        if !left.is_zero() {
            out.short.push(format!("{}: {}", l.name, left.normalize()));
        }
    }
    touch(&mut *conn, id).await?;
    Ok(out)
}

fn inventory(e: huxley_inventory::InventoryError) -> SalesError {
    match e {
        huxley_inventory::InventoryError::NotFound => SalesError::NotFound,
        huxley_inventory::InventoryError::Invalid { field, message } => {
            SalesError::Invalid { field, message }
        }
        huxley_inventory::InventoryError::Conflict(m) => SalesError::Conflict(m),
        huxley_inventory::InventoryError::Db(e) => SalesError::Db(e),
        huxley_inventory::InventoryError::Sqlx(e) => SalesError::Sqlx(e),
    }
}

// ---- by purchase ----

/// What a purchasing pass did.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesOrderPurchased)]
pub struct Purchased {
    /// Purchase orders drafted.
    pub orders: Vec<Uuid>,
    /// Lines short with no vendor to buy from (the catalog product's vendor
    /// matches none): `name: how many`.
    pub unmatched: Vec<String>,
}

/// Drafts purchase orders, per vendor, for what the order's stocked lines
/// are still short, bought for the order. Run in system scope: the stock
/// (and its vendors) are the provider's; the caller checks access there.
pub async fn purchase(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> SalesResult<Purchased> {
    let o = open(&mut *conn, id, None).await?;
    let mut groups: Vec<(Uuid, Uuid, Vec<huxley_inventory::orders::LineInput>)> = Vec::new();
    let mut out = Purchased::default();
    for l in &o.lines {
        let Some(item) = l.item_id else { continue };
        let need = short(l);
        if need.is_zero() {
            continue;
        }
        let vendor: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT pv.id, i.org_id FROM inventory_items i JOIN catalog_products p ON p.id = i.product_id \
               JOIN procurement_vendors pv ON pv.org_id = i.org_id AND pv.active AND lower(pv.name) = lower(p.vendor) \
              WHERE i.id = $1",
        )
        .bind(item)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((vendor, buyer)) = vendor else {
            out.unmatched
                .push(format!("{}: {}", l.name, need.normalize()));
            continue;
        };
        let line = huxley_inventory::orders::LineInput {
            item_id: Some(item),
            sku: None,
            name: None,
            quantity: if l.tracking == Some(Tracking::Serial) {
                need.ceil()
            } else {
                need
            },
            unit_cost: (l.unit_cost > Decimal::ZERO).then_some(l.unit_cost),
            for_ref_type: Some(RefType::SalesOrder),
            for_ref_id: Some(o.id),
        };
        match groups.iter_mut().find(|(v, _, _)| *v == vendor) {
            Some(g) => g.2.push(line),
            None => groups.push((vendor, buyer, vec![line])),
        }
    }
    for (vendor, buyer, lines) in groups {
        let po = huxley_inventory::orders::create(
            &mut *conn,
            ctx,
            buyer,
            &huxley_inventory::orders::OrderInput {
                vendor_id: vendor,
                currency: None,
                expected_on: None,
                ship_to: None,
                vendor_reference: None,
                notes: Some(format!("For SO-{} ({}).", o.number, o.org_name)),
                lines,
                version: None,
            },
        )
        .await
        .map_err(inventory)?;
        out.orders.push(po.id);
    }
    touch(&mut *conn, id).await?;
    Ok(out)
}

// ---- delivering ----

/// Delivers what's ready: stocked lines use their allocations (as far as
/// they go); lines with nothing to draw from are delivered in full;
/// recurring lines wait for their agreement. Fulfils the order when every
/// line is.
pub async fn fulfil(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
) -> SalesResult<Order> {
    let o = open(&mut *conn, id, Some(version)).await?;
    for l in &o.lines {
        let left = l.quantity - l.fulfilled;
        if left <= Decimal::ZERO || l.billing != Billing::OneTime {
            continue;
        }
        let delivered = match l.item_id {
            None => left,
            Some(item) => {
                let open: Vec<(Uuid, Decimal)> = sqlx::query_as(
                    "SELECT id, quantity FROM inventory_allocations WHERE ref_type = 'sales_order' AND ref_id = $1 \
                       AND item_id = $2 AND status = 'allocated' ORDER BY created_at",
                )
                .bind(o.id)
                .bind(item)
                .fetch_all(&mut *conn)
                .await?;
                let mut used = Decimal::ZERO;
                for (allocation, quantity) in open {
                    if used + quantity > left {
                        break;
                    }
                    stock::close(&mut *conn, ctx, allocation, AllocationStatus::Consumed)
                        .await
                        .map_err(inventory)?;
                    used += quantity;
                }
                used
            }
        };
        if delivered > Decimal::ZERO {
            sqlx::query("UPDATE sales_order_lines SET fulfilled = fulfilled + $2 WHERE id = $1")
                .bind(l.id)
                .bind(delivered)
                .execute(&mut *conn)
                .await?;
        }
    }
    touch(&mut *conn, id).await?;
    settle(conn, ctx, id).await
}

/// Fulfils the order when every line is.
async fn settle(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<Order> {
    let done: bool = sqlx::query_scalar(
        "SELECT coalesce(bool_and(fulfilled >= quantity), true) FROM sales_order_lines WHERE order_id = $1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if done {
        sqlx::query("UPDATE sales_orders SET status = 'fulfilled', updated_at = now() WHERE id = $1 AND status = 'open'")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        let o = get(&mut *conn, id).await?;
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("sales_order.fulfilled")
                .org(o.org_id)
                .target("sales_order", id)
                .details(json!({ "number": o.number })),
        )
        .await?;
        emit(&mut *conn, "sales_order.fulfilled", &o).await?;
        return Ok(o);
    }
    get(conn, id).await
}

/// Records the agreement made from the order's recurring lines (which are
/// then fulfilled).
pub async fn set_agreement(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    agreement: Uuid,
) -> SalesResult<Order> {
    let o = open(&mut *conn, id, None).await?;
    if o.agreement_id.is_some() {
        return Err(SalesError::Conflict("it already has an agreement".into()));
    }
    sqlx::query("UPDATE sales_orders SET agreement_id = $2 WHERE id = $1")
        .bind(id)
        .bind(agreement)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE sales_order_lines SET fulfilled = quantity WHERE order_id = $1 AND billing <> 'one_time'")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_order.agreement")
            .org(o.org_id)
            .target("sales_order", id)
            .details(json!({ "agreement": agreement })),
    )
    .await?;
    touch(&mut *conn, id).await?;
    settle(conn, ctx, id).await
}

/// Records the project started for the order.
pub async fn set_project(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    project: Uuid,
) -> SalesResult<Order> {
    let o = get(&mut *conn, id).await?;
    if o.project_id.is_some() {
        return Err(SalesError::Conflict("it already has a project".into()));
    }
    if matches!(o.status, Status::Cancelled | Status::Closed) {
        return Err(SalesError::Conflict("the order is finished".into()));
    }
    sqlx::query("UPDATE sales_orders SET project_id = $2 WHERE id = $1")
        .bind(id)
        .bind(project)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_order.project")
            .org(o.org_id)
            .target("sales_order", id)
            .details(json!({ "project": project })),
    )
    .await?;
    touch(&mut *conn, id).await?;
    get(conn, id).await
}

/// The recurring lines, for an agreement.
pub fn recurring(o: &Order) -> Vec<&Line> {
    o.lines
        .iter()
        .filter(|l| l.billing != Billing::OneTime)
        .collect()
}

/// Closes a fulfilled order, or cancels an open one that nothing was
/// delivered from (releasing what it had set aside).
pub async fn finish(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
    cancel: bool,
    note: Option<&str>,
) -> SalesResult<Order> {
    let (status, current): (Status, i32) =
        sqlx::query_as("SELECT status, version FROM sales_orders WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(SalesError::NotFound)?;
    if current != version {
        return Err(SalesError::VersionConflict);
    }
    let note = text("note", note, 4000)?;
    let o = get(&mut *conn, id).await?;
    if cancel {
        if status != Status::Open || o.lines.iter().any(|l| l.fulfilled > Decimal::ZERO) {
            return Err(SalesError::Conflict(
                "only an open order with nothing delivered can be cancelled".into(),
            ));
        }
        let held: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM inventory_allocations WHERE ref_type = 'sales_order' AND ref_id = $1 AND status = 'allocated'",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await?;
        for a in held {
            stock::close(&mut *conn, ctx, a, AllocationStatus::Released)
                .await
                .map_err(inventory)?;
        }
    } else if status != Status::Fulfilled {
        return Err(SalesError::Conflict("close it once it's fulfilled".into()));
    }
    sqlx::query(
        "UPDATE sales_orders SET status = $2, closed_at = now(), notes = coalesce($3, notes), \
            version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(if cancel {
        Status::Cancelled
    } else {
        Status::Closed
    })
    .bind(&note)
    .execute(&mut *conn)
    .await?;
    let after = get(&mut *conn, id).await?;
    let event = if cancel {
        "sales_order.cancelled"
    } else {
        "sales_order.closed"
    };
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(o.org_id)
            .target("sales_order", id)
            .details(json!({ "number": o.number, "note": note })),
    )
    .await?;
    emit(&mut *conn, event, &after).await?;
    Ok(after)
}
