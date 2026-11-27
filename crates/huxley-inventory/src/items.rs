//! Items: catalog products an organization stocks, how they're tracked,
//! where they are, and their reorder points.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{InventoryError, InventoryResult};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Tracking {
    /// By quantity only.
    #[default]
    None,
    /// Each unit by its serial number.
    Serial,
    /// By lot (batch) number.
    Lot,
}

/// A stocked item, with its totals across locations.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryItem)]
pub struct Item {
    pub id: Uuid,
    pub org_id: Uuid,
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub unit: String,
    pub tracking: Tracking,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub on_hand: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub allocated: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub available: Decimal,
    /// At or under a reorder point somewhere.
    pub low: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT: &str = "SELECT i.id, i.org_id, i.product_id, i.sku, i.name, i.unit, i.tracking, \
        coalesce(s.on_hand, 0) AS on_hand, coalesce(s.allocated, 0) AS allocated, \
        coalesce(s.on_hand, 0) - coalesce(s.allocated, 0) AS available, \
        EXISTS (SELECT 1 FROM inventory_levels v WHERE v.item_id = i.id AND v.low_since IS NOT NULL) AS low, \
        i.created_at \
   FROM inventory_items i \
   LEFT JOIN (SELECT item_id, sum(on_hand) AS on_hand, sum(allocated) AS allocated \
                FROM inventory_stock GROUP BY item_id) s ON s.item_id = i.id";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    /// SKU or name contains.
    pub q: Option<String>,
    /// Only items at or under a reorder point.
    #[serde(default)]
    pub low: bool,
    /// Only items with stock at this location.
    pub location_id: Option<Uuid>,
}

pub async fn list(conn: &mut PgConnection, filter: &Filter) -> InventoryResult<Vec<Item>> {
    let q = filter
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| {
            format!(
                "%{}%",
                q.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({SELECT}) x \
          WHERE ($1::text IS NULL OR x.sku ILIKE $1 OR x.name ILIKE $1) AND (NOT $2 OR x.low) \
            AND ($3::uuid IS NULL OR EXISTS (SELECT 1 FROM inventory_stock s \
                 WHERE s.item_id = x.id AND s.location_id = $3 AND s.on_hand > 0)) \
          ORDER BY x.name, x.sku LIMIT 500"
    )))
    .bind(q)
    .bind(filter.low)
    .bind(filter.location_id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Item> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE i.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(InventoryError::NotFound)
}

/// Where an item is stocked (for checking access there).
pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM inventory_items WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InventoryItemInput)]
pub struct ItemInput {
    /// A catalog product (a `product`, not a service or bundle) of this
    /// organization or one above it.
    pub product_id: Uuid,
    #[serde(default)]
    pub tracking: Tracking,
}

/// Starts stocking a product. Run in system scope: the product may be an
/// ancestor's; the caller checks access at `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &ItemInput,
) -> InventoryResult<Item> {
    let product: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT p.sku, p.name, p.unit, p.kind FROM catalog_products p \
           JOIN organizations po ON po.id = p.org_id \
           JOIN organizations o ON o.id = $2 \
          WHERE p.id = $1 AND po.path @> o.path",
    )
    .bind(input.product_id)
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((sku, name, unit, kind)) = product else {
        return Err(InventoryError::invalid(
            "productId",
            "not a catalog product here",
        ));
    };
    if kind != "product" {
        return Err(InventoryError::invalid(
            "productId",
            "only products are stocked, not services or bundles",
        ));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM inventory_items WHERE org_id = $1 AND product_id = $2)",
    )
    .bind(org_id)
    .bind(input.product_id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(InventoryError::Conflict("it's already stocked".into()));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO inventory_items (id, org_id, product_id, sku, name, unit, tracking) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.product_id)
    .bind(&sku)
    .bind(&name)
    .bind(&unit)
    .bind(input.tracking)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("inventory_item.created")
            .org(org_id)
            .target("inventory_item", id)
            .details(json!({ "sku": sku, "tracking": input.tracking })),
    )
    .await?;
    get(conn, id).await
}

/// Changes how an item is tracked (only while none is on hand) and
/// refreshes its SKU, name and unit from the catalog. Run in system scope.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    tracking: Tracking,
) -> InventoryResult<Item> {
    let current = get(&mut *conn, id).await?;
    if current.tracking != tracking && current.on_hand > Decimal::ZERO {
        return Err(InventoryError::Conflict(
            "change how it's tracked once none is on hand".into(),
        ));
    }
    sqlx::query(
        "UPDATE inventory_items i SET tracking = $2, sku = p.sku, name = p.name, unit = p.unit \
           FROM catalog_products p WHERE i.id = $1 AND p.id = i.product_id",
    )
    .bind(id)
    .bind(tracking)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("inventory_item.updated")
            .org(current.org_id)
            .target("inventory_item", id)
            .details(json!({ "tracking": tracking })),
    )
    .await?;
    get(conn, id).await
}

/// Stops stocking an item that has no stock and no history.
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> InventoryResult<()> {
    let current = get(&mut *conn, id).await?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inventory_movements WHERE item_id = $1)")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if used {
        return Err(InventoryError::Conflict(
            "it has stock history, so it stays".into(),
        ));
    }
    sqlx::query("DELETE FROM inventory_items WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("inventory_item.deleted")
            .org(current.org_id)
            .target("inventory_item", id)
            .details(json!({ "sku": current.sku })),
    )
    .await?;
    Ok(())
}

// ---- where it is ----

/// An item at a location: on hand (by lot), allocated, and the reorder point.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryStockLevel)]
pub struct Level {
    pub location_id: Uuid,
    pub location_name: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub on_hand: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub allocated: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub available: Decimal,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub reorder_point: Option<Decimal>,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub reorder_qty: Option<Decimal>,
    pub low: bool,
    /// Lot-tracked items: what's on hand of each lot.
    pub lots: Vec<Lot>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryLot)]
pub struct Lot {
    pub lot: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub on_hand: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub allocated: Decimal,
}

#[derive(sqlx::FromRow)]
struct LevelRow {
    location_id: Uuid,
    location_name: String,
    lot: Option<String>,
    on_hand: Decimal,
    allocated: Decimal,
    reorder_point: Option<Decimal>,
    reorder_qty: Option<Decimal>,
    low: bool,
}

/// Where an item is, and its reorder points (locations with either).
pub async fn levels(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Vec<Level>> {
    let rows: Vec<LevelRow> = sqlx::query_as(
        "SELECT l.id AS location_id, l.name AS location_name, s.lot, coalesce(s.on_hand, 0) AS on_hand, \
                coalesce(s.allocated, 0) AS allocated, v.reorder_point, v.reorder_qty, \
                v.low_since IS NOT NULL AS low \
           FROM inventory_locations l \
           LEFT JOIN inventory_stock s ON s.location_id = l.id AND s.item_id = $1 \
           LEFT JOIN inventory_levels v ON v.location_id = l.id AND v.item_id = $1 \
          WHERE s.id IS NOT NULL OR v.item_id IS NOT NULL \
          ORDER BY l.name, s.lot",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    let mut out: Vec<Level> = Vec::new();
    for r in rows {
        if out.last().is_none_or(|l| l.location_id != r.location_id) {
            out.push(Level {
                location_id: r.location_id,
                location_name: r.location_name,
                on_hand: Decimal::ZERO,
                allocated: Decimal::ZERO,
                available: Decimal::ZERO,
                reorder_point: r.reorder_point,
                reorder_qty: r.reorder_qty,
                low: r.low,
                lots: Vec::new(),
            });
        }
        let level = out.last_mut().expect("just pushed");
        level.on_hand += r.on_hand;
        level.allocated += r.allocated;
        level.available = level.on_hand - level.allocated;
        if let Some(lot) = r.lot.filter(|l| !l.is_empty()) {
            level.lots.push(Lot {
                lot,
                on_hand: r.on_hand,
                allocated: r.allocated,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InventoryReorderInput)]
pub struct ReorderInput {
    /// Raise `stock.low` when what's available falls to this.
    #[serde(deserialize_with = "crate::number")]
    #[schema(value_type = String)]
    pub reorder_point: Decimal,
    /// How many to order then (a suggestion for purchase orders).
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub reorder_qty: Option<Decimal>,
}

/// Sets an item's reorder point at a location (of the item's organization).
pub async fn set_reorder(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    location: Uuid,
    input: &ReorderInput,
) -> InventoryResult<()> {
    let item = get(&mut *conn, id).await?;
    crate::stock::location_of(&mut *conn, &item, location, "locationId").await?;
    if input.reorder_point < Decimal::ZERO || input.reorder_qty.is_some_and(|q| q < Decimal::ZERO) {
        return Err(InventoryError::invalid("reorderPoint", "can't be negative"));
    }
    sqlx::query(
        "INSERT INTO inventory_levels (item_id, location_id, org_id, reorder_point, reorder_qty) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (item_id, location_id) DO UPDATE SET reorder_point = $4, reorder_qty = $5",
    )
    .bind(id)
    .bind(location)
    .bind(item.org_id)
    .bind(input.reorder_point)
    .bind(input.reorder_qty.unwrap_or_default())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("inventory_reorder.set")
            .org(item.org_id)
            .target("inventory_item", id)
            .details(json!({ "location": location, "reorderPoint": input.reorder_point.normalize().to_string() })),
    )
    .await?;
    crate::stock::check_low(conn, &item, location).await
}

/// Clears an item's reorder point at a location.
pub async fn clear_reorder(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    location: Uuid,
) -> InventoryResult<()> {
    let item = get(&mut *conn, id).await?;
    let done = sqlx::query("DELETE FROM inventory_levels WHERE item_id = $1 AND location_id = $2")
        .bind(id)
        .bind(location)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Err(InventoryError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("inventory_reorder.cleared")
            .org(item.org_id)
            .target("inventory_item", id)
            .details(json!({ "location": location })),
    )
    .await?;
    Ok(())
}

// ---- serial numbers ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InventoryUnitStatus)]
pub enum UnitStatus {
    InStock,
    Allocated,
    Consumed,
    /// Sent back to the vendor.
    Rma,
    /// Returned and written off.
    Returned,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryUnit)]
pub struct Unit {
    pub id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub serial: String,
    pub status: UnitStatus,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    pub allocation_id: Option<Uuid>,
    /// What it's allocated to or was used on.
    pub ref_type: Option<String>,
    pub ref_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct UnitFilter {
    pub item_id: Option<Uuid>,
    /// Serial number contains.
    pub serial: Option<String>,
    /// Only units in stock (or allocated).
    #[serde(default)]
    pub in_stock: bool,
}

pub async fn units(conn: &mut PgConnection, filter: &UnitFilter) -> InventoryResult<Vec<Unit>> {
    let serial = filter
        .serial
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            format!(
                "%{}%",
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    Ok(sqlx::query_as(
        "SELECT u.id, u.item_id, i.sku, i.name, u.serial, u.status, u.location_id, l.name AS location_name, \
                u.allocation_id, a.ref_type, a.ref_id, u.updated_at \
           FROM inventory_units u JOIN inventory_items i ON i.id = u.item_id \
           LEFT JOIN inventory_locations l ON l.id = u.location_id \
           LEFT JOIN inventory_allocations a ON a.id = u.allocation_id \
          WHERE ($1::uuid IS NULL OR u.item_id = $1) AND ($2::text IS NULL OR u.serial ILIKE $2) \
            AND (NOT $3 OR u.status IN ('in_stock', 'allocated')) \
          ORDER BY u.serial LIMIT 500",
    )
    .bind(filter.item_id)
    .bind(serial)
    .bind(filter.in_stock)
    .fetch_all(conn)
    .await?)
}
