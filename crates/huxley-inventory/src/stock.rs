//! Moving stock: the ledger, allocations to tickets and projects, serial
//! units, and reorder alerts.
//!
//! Every change goes through [`bump`], which locks the stock row and keeps
//! on hand and allocated from going negative (or allocated above on hand),
//! and ends with [`check_low`] for the locations it touched.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::items::{self, Item, Tracking, UnitStatus};
use crate::locations::Kind;
use crate::{InventoryError, InventoryResult, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InventoryMovementKind)]
pub enum MovementKind {
    /// Into stock: delivered, bought, found.
    Receive,
    /// From one location to another.
    Transfer,
    /// A correction after a count: up or down, with why.
    Adjust,
    /// Set aside for a ticket or project.
    Allocate,
    /// An allocation given back to stock.
    Release,
    /// An allocation used.
    Consume,
    /// Back into stock from a job.
    Return,
    /// Out to the vendor under a return authorization.
    Rma,
}

impl MovementKind {
    const fn event(self) -> &'static str {
        match self {
            Self::Receive => "inventory.received",
            Self::Transfer => "inventory.transferred",
            Self::Adjust => "inventory.adjusted",
            Self::Allocate => "inventory.allocated",
            Self::Release => "inventory.released",
            Self::Consume => "inventory.consumed",
            Self::Return => "inventory.returned",
            Self::Rma => "inventory.rma",
        }
    }
}

// ---- the building blocks ----

/// A location of the item's organization, active, for stock to be in.
pub(crate) async fn location_of(
    conn: &mut PgConnection,
    item: &Item,
    location: Uuid,
    field: &str,
) -> InventoryResult<String> {
    let found: Option<(Uuid, String, bool, Kind)> =
        sqlx::query_as("SELECT org_id, name, active, kind FROM inventory_locations WHERE id = $1")
            .bind(location)
            .fetch_optional(conn)
            .await?;
    match found {
        Some((org, name, true, _)) if org == item.org_id => Ok(name),
        Some((org, _, false, _)) if org == item.org_id => Err(InventoryError::invalid(
            field,
            "that location is turned off",
        )),
        _ => Err(InventoryError::invalid(
            field,
            "not a location of this item's organization",
        )),
    }
}

/// Changes on hand and allocated for an item at a location and lot,
/// refusing to go below zero (or allocate more than is on hand).
async fn bump(
    conn: &mut PgConnection,
    item: &Item,
    location: Uuid,
    lot: &str,
    on_hand: Decimal,
    allocated: Decimal,
) -> InventoryResult<()> {
    sqlx::query(
        "INSERT INTO inventory_stock (id, org_id, item_id, location_id, lot) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (item_id, location_id, lot) DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(item.org_id)
    .bind(item.id)
    .bind(location)
    .bind(lot)
    .execute(&mut *conn)
    .await?;
    let (id, have, held): (Uuid, Decimal, Decimal) = sqlx::query_as(
        "SELECT id, on_hand, allocated FROM inventory_stock \
          WHERE item_id = $1 AND location_id = $2 AND lot = $3 FOR UPDATE",
    )
    .bind(item.id)
    .bind(location)
    .bind(lot)
    .fetch_one(&mut *conn)
    .await?;
    let (next, next_held) = (have + on_hand, held + allocated);
    if next < Decimal::ZERO || next_held < Decimal::ZERO || next_held > next {
        let lot = if lot.is_empty() {
            String::new()
        } else {
            format!(" of lot {lot}")
        };
        return Err(InventoryError::Conflict(format!(
            "only {} {}{lot} available there",
            (have - held).normalize(),
            item.unit
        )));
    }
    if next == Decimal::ZERO && next_held == Decimal::ZERO {
        sqlx::query("DELETE FROM inventory_stock WHERE id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    } else {
        sqlx::query("UPDATE inventory_stock SET on_hand = $2, allocated = $3, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(next)
            .bind(next_held)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Raises `stock.low` when what's available at a location falls to its
/// reorder point (once per dip), and clears the mark when it's back above.
pub(crate) async fn check_low(
    conn: &mut PgConnection,
    item: &Item,
    location: Uuid,
) -> InventoryResult<()> {
    let level: Option<(Decimal, Decimal, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT reorder_point, reorder_qty, low_since FROM inventory_levels \
          WHERE item_id = $1 AND location_id = $2 FOR UPDATE",
    )
    .bind(item.id)
    .bind(location)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((point, qty, low_since)) = level else {
        return Ok(());
    };
    let available: Decimal = sqlx::query_scalar(
        "SELECT coalesce(sum(on_hand - allocated), 0) FROM inventory_stock WHERE item_id = $1 AND location_id = $2",
    )
    .bind(item.id)
    .bind(location)
    .fetch_one(&mut *conn)
    .await?;
    let low = available <= point;
    if low == low_since.is_some() {
        return Ok(());
    }
    sqlx::query("UPDATE inventory_levels SET low_since = CASE WHEN $3 THEN now() END WHERE item_id = $1 AND location_id = $2")
        .bind(item.id)
        .bind(location)
        .bind(low)
        .execute(&mut *conn)
        .await?;
    if low {
        let name: String = sqlx::query_scalar("SELECT name FROM inventory_locations WHERE id = $1")
            .bind(location)
            .fetch_one(&mut *conn)
            .await?;
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "stock.low",
                org_id: Some(item.org_id),
                subject_type: "inventory_item",
                subject_id: item.id,
                payload: json!({
                    "itemId": item.id, "productId": item.product_id, "sku": item.sku, "name": item.name,
                    "locationId": location, "location": name,
                    "available": available.normalize().to_string(),
                    "reorderPoint": point.normalize().to_string(),
                    "reorderQty": qty.normalize().to_string(),
                    "link": format!("/inventory/items/{}", item.id),
                }),
            },
        )
        .await?;
    }
    Ok(())
}

/// Serial numbers given: trimmed, without blanks or repeats.
fn serial_list(serials: &[String]) -> InventoryResult<Vec<String>> {
    let mut out: Vec<String> = Vec::with_capacity(serials.len());
    for s in serials.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if s.chars().count() > 100 {
            return Err(InventoryError::invalid(
                "serials",
                "at most 100 characters each",
            ));
        }
        if out.iter().any(|o| o == s) {
            return Err(InventoryError::invalid(
                "serials",
                format!("{s} is listed twice"),
            ));
        }
        out.push(s.to_owned());
    }
    Ok(out)
}

/// The quantity, lot and serials a movement of `item` needs: serial items
/// move whole units, named; lot items name the lot; others neither.
fn shape(
    item: &Item,
    quantity: Option<Decimal>,
    lot: Option<&str>,
    serials: &[String],
) -> InventoryResult<(Decimal, String, Vec<String>)> {
    let serials = serial_list(serials)?;
    let lot = text("lot", lot, 100)?.unwrap_or_default();
    match item.tracking {
        Tracking::Serial => {
            if serials.is_empty() {
                return Err(InventoryError::invalid(
                    "serials",
                    "name the serial numbers",
                ));
            }
            let n = Decimal::from(serials.len());
            if quantity.is_some_and(|q| q.abs() != n) {
                return Err(InventoryError::invalid(
                    "quantity",
                    format!(
                        "{} serial numbers for a quantity of {}",
                        n,
                        quantity.unwrap_or_default().normalize()
                    ),
                ));
            }
            let sign = if quantity.is_some_and(|q| q < Decimal::ZERO) {
                -Decimal::ONE
            } else {
                Decimal::ONE
            };
            Ok((n * sign, String::new(), serials))
        }
        Tracking::Lot | Tracking::None => {
            if !serials.is_empty() {
                return Err(InventoryError::invalid(
                    "serials",
                    "this item isn't tracked by serial number",
                ));
            }
            if item.tracking == Tracking::Lot && lot.is_empty() {
                return Err(InventoryError::invalid("lot", "name the lot"));
            }
            let q = quantity.ok_or_else(|| InventoryError::invalid("quantity", "required"))?;
            if q == Decimal::ZERO {
                return Err(InventoryError::invalid("quantity", "can't be zero"));
            }
            let lot = if item.tracking == Tracking::Lot {
                lot
            } else {
                String::new()
            };
            Ok((q, lot, serials))
        }
    }
}

/// The units with these serials, checking each is `status` at `location`
/// (`None`: anywhere). Returns their ids.
async fn units(
    conn: &mut PgConnection,
    item: &Item,
    serials: &[String],
    status: &[UnitStatus],
    location: Option<Uuid>,
) -> InventoryResult<Vec<Uuid>> {
    let found: Vec<(Uuid, String, UnitStatus, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, serial, status, location_id FROM inventory_units \
          WHERE item_id = $1 AND serial = ANY($2) FOR UPDATE",
    )
    .bind(item.id)
    .bind(serials)
    .fetch_all(&mut *conn)
    .await?;
    let mut ids = Vec::with_capacity(serials.len());
    for s in serials {
        match found.iter().find(|(_, serial, _, _)| serial == s) {
            Some((id, _, st, loc))
                if status.contains(st) && (location.is_none() || *loc == location) =>
            {
                ids.push(*id);
            }
            Some(_) => {
                return Err(InventoryError::invalid(
                    "serials",
                    format!("{s} isn't available there"),
                ));
            }
            None => {
                return Err(InventoryError::invalid(
                    "serials",
                    format!("{s} isn't known"),
                ));
            }
        }
    }
    Ok(ids)
}

async fn set_units(
    conn: &mut PgConnection,
    ids: &[Uuid],
    status: UnitStatus,
    location: Option<Uuid>,
    allocation: Option<Uuid>,
) -> InventoryResult<()> {
    sqlx::query(
        "UPDATE inventory_units SET status = $2, location_id = $3, allocation_id = $4, updated_at = now() \
          WHERE id = ANY($1)",
    )
    .bind(ids)
    .bind(status)
    .bind(location)
    .bind(allocation)
    .execute(conn)
    .await?;
    Ok(())
}

/// A ledger entry.
struct Entry<'a> {
    kind: MovementKind,
    quantity: Decimal,
    from: Option<Uuid>,
    to: Option<Uuid>,
    lot: &'a str,
    serials: &'a [String],
    unit_cost: Option<Decimal>,
    allocation: Option<Uuid>,
    reference: (Option<&'a str>, Option<Uuid>),
    label: Option<&'a str>,
    note: Option<&'a str>,
}

async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    item: &Item,
    e: Entry<'_>,
) -> InventoryResult<Uuid> {
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO inventory_movements (id, org_id, item_id, kind, quantity, from_location, to_location, lot, \
            serials, unit_cost, allocation_id, ref_type, ref_id, reference, note, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(id)
    .bind(item.org_id)
    .bind(item.id)
    .bind(e.kind)
    .bind(e.quantity)
    .bind(e.from)
    .bind(e.to)
    .bind(e.lot)
    .bind(e.serials)
    .bind(e.unit_cost)
    .bind(e.allocation)
    .bind(e.reference.0)
    .bind(e.reference.1)
    .bind(e.label)
    .bind(e.note)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let details = json!({
        "movement": id, "sku": item.sku, "quantity": e.quantity.normalize().to_string(),
        "from": e.from, "to": e.to, "lot": e.lot, "serials": e.serials,
        "refType": e.reference.0, "refId": e.reference.1, "reference": e.label,
    });
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(e.kind.event())
            .org(item.org_id)
            .target("inventory_item", item.id)
            .details(details.clone()),
    )
    .await?;
    let mut payload = details;
    payload["itemId"] = json!(item.id);
    payload["productId"] = json!(item.product_id);
    payload["name"] = json!(item.name);
    payload["allocationId"] = json!(e.allocation);
    payload["unitCost"] = json!(e.unit_cost.map(|c| c.normalize().to_string()));
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: e.kind.event(),
            org_id: Some(item.org_id),
            subject_type: "inventory_item",
            subject_id: item.id,
            payload,
        },
    )
    .await?;
    Ok(id)
}

// ---- movements ----

/// A stock movement to make: receive, transfer, adjust, return or RMA.
/// (Allocating, releasing and consuming go through allocations.)
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InventoryMovementInput)]
pub struct MovementInput {
    pub kind: MovementKind,
    pub item_id: Uuid,
    /// Where: received into, adjusted, returned into, sent back from; a
    /// transfer's source.
    pub location_id: Uuid,
    /// A transfer's destination.
    #[serde(default)]
    pub to_location_id: Option<Uuid>,
    /// How many (signed for an adjustment). Serial items: the serial
    /// numbers count.
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub quantity: Option<Decimal>,
    #[serde(default)]
    pub lot: Option<String>,
    #[serde(default)]
    pub serials: Vec<String>,
    /// Receiving: what each cost.
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_cost: Option<Decimal>,
    /// A delivery note, PO or RMA number.
    #[serde(default)]
    pub reference: Option<String>,
    /// Why (required for adjustments).
    #[serde(default)]
    pub note: Option<String>,
}

/// Makes a movement and returns it.
pub async fn movement(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &MovementInput,
) -> InventoryResult<Movement> {
    movement_for(conn, ctx, input, None).await
}

/// [`movement`], recording what it came from (e.g. `("purchase_order", id)`).
pub async fn movement_for(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &MovementInput,
    source: Option<(&str, Uuid)>,
) -> InventoryResult<Movement> {
    use MovementKind::{Adjust, Allocate, Consume, Receive, Release, Return, Rma, Transfer};
    let item = items::get(&mut *conn, input.item_id).await?;
    let at = input.location_id;
    location_of(&mut *conn, &item, at, "locationId").await?;
    let (quantity, lot, serials) =
        shape(&item, input.quantity, input.lot.as_deref(), &input.serials)?;
    let reference = text("reference", input.reference.as_deref(), 200)?;
    let note = text("note", input.note.as_deref(), 1000)?;
    if input.kind != Adjust && quantity < Decimal::ZERO {
        return Err(InventoryError::invalid(
            "quantity",
            "must be more than zero",
        ));
    }
    if input.kind == Adjust && note.is_none() {
        return Err(InventoryError::invalid("note", "say why"));
    }
    if input.unit_cost.is_some_and(|c| c < Decimal::ZERO) {
        return Err(InventoryError::invalid("unitCost", "can't be negative"));
    }
    let dest = destination(&mut *conn, &item, input).await?;
    let into = quantity > Decimal::ZERO;
    // Where it goes from and to, and how on hand changes where it is.
    let (from, to, change) = match input.kind {
        Receive | Return => (None, Some(at), quantity),
        Rma => (Some(at), None, -quantity),
        Transfer => (Some(at), dest, -quantity),
        Adjust if into => (None, Some(at), quantity),
        Adjust => (Some(at), None, quantity),
        Allocate | Release | Consume => {
            return Err(InventoryError::invalid(
                "kind",
                "allocate parts through allocations",
            ));
        }
    };
    move_units(&mut *conn, &item, input.kind, into, &serials, at, dest).await?;
    bump(&mut *conn, &item, at, &lot, change, Decimal::ZERO).await?;
    if let Some(dest) = dest {
        bump(&mut *conn, &item, dest, &lot, quantity, Decimal::ZERO).await?;
    }
    let entry = Entry {
        kind: input.kind,
        quantity,
        from,
        to,
        lot: &lot,
        serials: &serials,
        unit_cost: if input.kind == Receive {
            input.unit_cost
        } else {
            None
        },
        allocation: None,
        reference: (source.map(|s| s.0), source.map(|s| s.1)),
        label: reference.as_deref(),
        note: note.as_deref(),
    };
    let id = record(&mut *conn, ctx, &item, entry).await?;
    check_low(&mut *conn, &item, at).await?;
    if let Some(dest) = dest {
        check_low(&mut *conn, &item, dest).await?;
    }
    get_movement(conn, id).await
}

/// A transfer's destination (checked); none for other movements.
async fn destination(
    conn: &mut PgConnection,
    item: &Item,
    input: &MovementInput,
) -> InventoryResult<Option<Uuid>> {
    if input.kind != MovementKind::Transfer {
        return Ok(None);
    }
    let to = input
        .to_location_id
        .ok_or_else(|| InventoryError::invalid("toLocationId", "required"))?;
    if to == input.location_id {
        return Err(InventoryError::invalid(
            "toLocationId",
            "pick another location",
        ));
    }
    location_of(conn, item, to, "toLocationId").await?;
    Ok(Some(to))
}

/// What a movement does to serial units.
async fn move_units(
    conn: &mut PgConnection,
    item: &Item,
    kind: MovementKind,
    into: bool,
    serials: &[String],
    at: Uuid,
    dest: Option<Uuid>,
) -> InventoryResult<()> {
    use UnitStatus::{Consumed, InStock, Returned, Rma};
    match kind {
        MovementKind::Receive => receive_units(conn, item, serials, at).await,
        MovementKind::Adjust if into => receive_units(conn, item, serials, at).await,
        MovementKind::Adjust => {
            shift(conn, item, serials, &[InStock], Some(at), Returned, None).await
        }
        MovementKind::Transfer => {
            shift(conn, item, serials, &[InStock], Some(at), InStock, dest).await
        }
        MovementKind::Return => {
            shift(conn, item, serials, &[Consumed], None, InStock, Some(at)).await
        }
        _ => shift(conn, item, serials, &[InStock], Some(at), Rma, None).await,
    }
}

/// Moves serial units that are `from` (at `at`, or anywhere) to `to` at
/// `location`. Nothing to do for items without serials.
async fn shift(
    conn: &mut PgConnection,
    item: &Item,
    serials: &[String],
    from: &[UnitStatus],
    at: Option<Uuid>,
    to: UnitStatus,
    location: Option<Uuid>,
) -> InventoryResult<()> {
    if serials.is_empty() {
        return Ok(());
    }
    let ids = units(&mut *conn, item, serials, from, at).await?;
    set_units(conn, &ids, to, location, None).await
}

/// New serial units in stock at a location (or ones that left stock, back).
async fn receive_units(
    conn: &mut PgConnection,
    item: &Item,
    serials: &[String],
    location: Uuid,
) -> InventoryResult<()> {
    for s in serials {
        let current: Option<UnitStatus> = sqlx::query_scalar(
            "SELECT status FROM inventory_units WHERE item_id = $1 AND serial = $2 FOR UPDATE",
        )
        .bind(item.id)
        .bind(s)
        .fetch_optional(&mut *conn)
        .await?;
        match current {
            Some(UnitStatus::InStock | UnitStatus::Allocated) => {
                return Err(InventoryError::invalid(
                    "serials",
                    format!("{s} is already in stock"),
                ));
            }
            Some(_) => {
                sqlx::query(
                    "UPDATE inventory_units SET status = 'in_stock', location_id = $3, allocation_id = NULL, \
                        updated_at = now() WHERE item_id = $1 AND serial = $2",
                )
                .bind(item.id)
                .bind(s)
                .bind(location)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO inventory_units (id, org_id, item_id, serial, location_id, status) \
                     VALUES ($1, $2, $3, $4, $5, 'in_stock')",
                )
                .bind(huxley_core::new_id())
                .bind(item.org_id)
                .bind(item.id)
                .bind(s)
                .bind(location)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryMovement)]
pub struct Movement {
    pub id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub kind: MovementKind,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub from_location_id: Option<Uuid>,
    pub from_location_name: Option<String>,
    pub to_location_id: Option<Uuid>,
    pub to_location_name: Option<String>,
    pub lot: String,
    pub serials: Vec<String>,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_cost: Option<Decimal>,
    pub allocation_id: Option<Uuid>,
    pub ref_type: Option<String>,
    pub ref_id: Option<Uuid>,
    pub reference: Option<String>,
    pub note: Option<String>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const MOVEMENT: &str = "SELECT m.id, m.item_id, i.sku, i.name, m.kind, m.quantity, \
        m.from_location AS from_location_id, f.name AS from_location_name, \
        m.to_location AS to_location_id, t.name AS to_location_name, m.lot, m.serials, m.unit_cost, \
        m.allocation_id, m.ref_type, m.ref_id, m.reference, m.note, u.display_name AS created_by_name, m.created_at \
   FROM inventory_movements m JOIN inventory_items i ON i.id = m.item_id \
   LEFT JOIN inventory_locations f ON f.id = m.from_location \
   LEFT JOIN inventory_locations t ON t.id = m.to_location \
   LEFT JOIN users u ON u.id = m.created_by";

async fn get_movement(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Movement> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{MOVEMENT} WHERE m.id = $1")))
            .bind(id)
            .fetch_one(conn)
            .await?,
    )
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct MovementFilter {
    pub item_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    /// `ticket` or `project`, with `refId`.
    pub ref_type: Option<String>,
    pub ref_id: Option<Uuid>,
    /// At most this many, newest first (default 100, at most 500).
    pub limit: Option<i64>,
}

pub async fn movements(
    conn: &mut PgConnection,
    filter: &MovementFilter,
) -> InventoryResult<Vec<Movement>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MOVEMENT} WHERE ($1::uuid IS NULL OR m.item_id = $1) \
            AND ($2::uuid IS NULL OR m.from_location = $2 OR m.to_location = $2) \
            AND ($3::text IS NULL OR m.ref_type = $3) AND ($4::uuid IS NULL OR m.ref_id = $4) \
          ORDER BY m.created_at DESC, m.id DESC LIMIT $5"
    )))
    .bind(filter.item_id)
    .bind(filter.location_id)
    .bind(filter.ref_type.as_deref())
    .bind(filter.ref_id)
    .bind(filter.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

// ---- allocations ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InventoryRefType)]
pub enum RefType {
    Ticket,
    Project,
}

impl RefType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ticket => "ticket",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InventoryAllocationStatus)]
pub enum AllocationStatus {
    Allocated,
    Consumed,
    Released,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryAllocation)]
pub struct Allocation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub unit: String,
    pub location_id: Uuid,
    pub location_name: String,
    pub lot: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub serials: Vec<String>,
    pub ref_type: RefType,
    pub ref_id: Uuid,
    pub ref_org_id: Option<Uuid>,
    pub status: AllocationStatus,
    pub note: Option<String>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
}

const ALLOCATION: &str = "SELECT a.id, a.org_id, a.item_id, i.sku, i.name, i.unit, a.location_id, \
        l.name AS location_name, a.lot, a.quantity, a.serials, a.ref_type, a.ref_id, a.ref_org_id, a.status, \
        a.note, u.display_name AS created_by_name, a.created_at, a.closed_at \
   FROM inventory_allocations a JOIN inventory_items i ON i.id = a.item_id \
   JOIN inventory_locations l ON l.id = a.location_id LEFT JOIN users u ON u.id = a.created_by";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct AllocationFilter {
    pub ref_type: Option<RefType>,
    pub ref_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    /// Only open allocations (not yet used or released).
    #[serde(default)]
    pub open: bool,
}

pub async fn allocations(
    conn: &mut PgConnection,
    filter: &AllocationFilter,
) -> InventoryResult<Vec<Allocation>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ALLOCATION} WHERE ($1::text IS NULL OR a.ref_type = $1) AND ($2::uuid IS NULL OR a.ref_id = $2) \
            AND ($3::uuid IS NULL OR a.item_id = $3) AND (NOT $4 OR a.status = 'allocated') \
          ORDER BY a.created_at DESC LIMIT 500"
    )))
    .bind(filter.ref_type.map(RefType::as_str))
    .bind(filter.ref_id)
    .bind(filter.item_id)
    .bind(filter.open)
    .fetch_all(conn)
    .await?)
}

pub async fn allocation(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Allocation> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ALLOCATION} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(InventoryError::NotFound)
}

/// Where an allocation's stock is (for checking access there).
pub async fn allocation_org(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM inventory_allocations WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// The client organization of a ticket or project, if it exists (in the
/// caller's scope).
pub async fn ref_org(
    conn: &mut PgConnection,
    kind: RefType,
    id: Uuid,
) -> InventoryResult<Option<Uuid>> {
    let sql = match kind {
        RefType::Ticket => "SELECT org_id FROM sd_tickets WHERE id = $1",
        RefType::Project => "SELECT org_id FROM prj_projects WHERE id = $1",
    };
    Ok(sqlx::query_scalar(sql)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InventoryAllocationInput)]
pub struct AllocationInput {
    pub item_id: Uuid,
    pub location_id: Uuid,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub quantity: Option<Decimal>,
    #[serde(default)]
    pub lot: Option<String>,
    #[serde(default)]
    pub serials: Vec<String>,
    pub ref_type: RefType,
    pub ref_id: Uuid,
    #[serde(default)]
    pub note: Option<String>,
    /// Used straight away (allocated and consumed in one go).
    #[serde(default)]
    pub consume: bool,
}

/// Sets parts aside for a ticket or project (`ref_org`: its client
/// organization, which the caller found).
pub async fn allocate(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &AllocationInput,
    ref_org: Uuid,
) -> InventoryResult<Allocation> {
    let item = items::get(&mut *conn, input.item_id).await?;
    let at = input.location_id;
    location_of(&mut *conn, &item, at, "locationId").await?;
    let (quantity, lot, serials) =
        shape(&item, input.quantity, input.lot.as_deref(), &input.serials)?;
    if quantity <= Decimal::ZERO {
        return Err(InventoryError::invalid(
            "quantity",
            "must be more than zero",
        ));
    }
    let note = text("note", input.note.as_deref(), 1000)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO inventory_allocations (id, org_id, item_id, location_id, lot, quantity, serials, ref_type, \
            ref_id, ref_org_id, note, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(item.org_id)
    .bind(item.id)
    .bind(at)
    .bind(&lot)
    .bind(quantity)
    .bind(&serials)
    .bind(input.ref_type)
    .bind(input.ref_id)
    .bind(ref_org)
    .bind(&note)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if !serials.is_empty() {
        let ids = units(
            &mut *conn,
            &item,
            &serials,
            &[UnitStatus::InStock],
            Some(at),
        )
        .await?;
        set_units(&mut *conn, &ids, UnitStatus::Allocated, Some(at), Some(id)).await?;
    }
    bump(&mut *conn, &item, at, &lot, Decimal::ZERO, quantity).await?;
    record(
        &mut *conn,
        ctx,
        &item,
        Entry {
            kind: MovementKind::Allocate,
            quantity,
            from: Some(at),
            to: None,
            lot: &lot,
            serials: &serials,
            unit_cost: None,
            allocation: Some(id),
            reference: (Some(input.ref_type.as_str()), Some(input.ref_id)),
            label: None,
            note: note.as_deref(),
        },
    )
    .await?;
    check_low(&mut *conn, &item, at).await?;
    if input.consume {
        return close(conn, ctx, id, AllocationStatus::Consumed).await;
    }
    allocation(conn, id).await
}

/// Uses an allocation (it leaves stock) or releases it (back to available).
pub async fn close(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    to: AllocationStatus,
) -> InventoryResult<Allocation> {
    let a = allocation(&mut *conn, id).await?;
    let status: AllocationStatus =
        sqlx::query_scalar("SELECT status FROM inventory_allocations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if status != AllocationStatus::Allocated {
        return Err(InventoryError::Conflict(
            "it was already used or released".into(),
        ));
    }
    let item = items::get(&mut *conn, a.item_id).await?;
    let (kind, on_hand) = match to {
        AllocationStatus::Consumed => (MovementKind::Consume, -a.quantity),
        AllocationStatus::Released => (MovementKind::Release, Decimal::ZERO),
        AllocationStatus::Allocated => {
            return Err(InventoryError::invalid("status", "use or release it"));
        }
    };
    bump(
        &mut *conn,
        &item,
        a.location_id,
        &a.lot,
        on_hand,
        -a.quantity,
    )
    .await?;
    if !a.serials.is_empty() {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM inventory_units WHERE allocation_id = $1")
                .bind(id)
                .fetch_all(&mut *conn)
                .await?;
        match to {
            AllocationStatus::Consumed => {
                set_units(&mut *conn, &ids, UnitStatus::Consumed, None, Some(id)).await?;
            }
            _ => {
                set_units(
                    &mut *conn,
                    &ids,
                    UnitStatus::InStock,
                    Some(a.location_id),
                    None,
                )
                .await?;
            }
        }
    }
    sqlx::query("UPDATE inventory_allocations SET status = $2, closed_at = now() WHERE id = $1")
        .bind(id)
        .bind(to)
        .execute(&mut *conn)
        .await?;
    let unit_cost: Option<Decimal> = if to == AllocationStatus::Consumed {
        // The latest cost paid, for billing and job costing.
        sqlx::query_scalar(
            "SELECT unit_cost FROM inventory_movements WHERE item_id = $1 AND kind = 'receive' \
               AND unit_cost IS NOT NULL ORDER BY created_at DESC LIMIT 1",
        )
        .bind(item.id)
        .fetch_optional(&mut *conn)
        .await?
    } else {
        None
    };
    record(
        &mut *conn,
        ctx,
        &item,
        Entry {
            kind,
            quantity: a.quantity,
            from: Some(a.location_id),
            to: None,
            lot: &a.lot,
            serials: &a.serials,
            unit_cost,
            allocation: Some(id),
            reference: (Some(a.ref_type.as_str()), Some(a.ref_id)),
            label: None,
            note: None,
        },
    )
    .await?;
    check_low(&mut *conn, &item, a.location_id).await?;
    allocation(conn, id).await
}
