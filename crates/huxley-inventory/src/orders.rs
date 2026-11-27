//! Purchase orders (ADR-068).
//!
//! Draft → submitted (when over the organization's approval threshold) →
//! approved → ordered → partially received → received; cancelled while
//! nothing has arrived. Only drafts change. Receiving puts stocked lines
//! into inventory (a `receive` movement referencing the order), and lines
//! bought for a ticket or project are allocated to it on arrival.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::items::Tracking;
use crate::stock::{self, AllocationInput, MovementInput, MovementKind, RefType};
use crate::{InventoryError, InventoryResult, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = PurchaseOrderStatus)]
pub enum Status {
    Draft,
    /// Waiting for approval.
    Submitted,
    Approved,
    /// Sent to the vendor.
    Ordered,
    PartiallyReceived,
    Received,
    Cancelled,
}

impl Status {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Submitted => "submitted",
            Self::Approved => "approved",
            Self::Ordered => "ordered",
            Self::PartiallyReceived => "partially_received",
            Self::Received => "received",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PurchaseOrderLine)]
pub struct Line {
    pub id: Uuid,
    pub item_id: Option<Uuid>,
    /// How the stocked item is tracked (none for a line of its own).
    pub tracking: Option<Tracking>,
    pub sku: Option<String>,
    pub name: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub unit_cost: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub received: Decimal,
    pub for_ref_type: Option<RefType>,
    pub for_ref_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PurchaseOrder)]
pub struct Order {
    pub id: Uuid,
    pub org_id: Uuid,
    pub number: i64,
    pub vendor_id: Uuid,
    pub vendor_name: String,
    pub vendor_email: Option<String>,
    pub status: Status,
    pub currency: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub expected_on: Option<Date>,
    pub ship_to: Option<Uuid>,
    pub ship_to_name: Option<String>,
    pub vendor_reference: Option<String>,
    pub notes: Option<String>,
    pub rejection: Option<String>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
    pub approved_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub approved_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ordered_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[sqlx(skip)]
    pub lines: Vec<Line>,
}

const SELECT: &str = "SELECT o.id, o.org_id, o.number, o.vendor_id, v.name AS vendor_name, v.email AS vendor_email, \
        o.status, o.currency, o.expected_on, o.ship_to, l.name AS ship_to_name, o.vendor_reference, o.notes, \
        o.rejection, c.display_name AS created_by_name, o.submitted_at, a.display_name AS approved_by_name, \
        o.approved_at, o.ordered_at, o.closed_at, \
        coalesce((SELECT sum(round(x.quantity * x.unit_cost, 2)) FROM procurement_order_lines x \
                   WHERE x.order_id = o.id), 0) AS total, \
        o.version, o.created_at, o.updated_at \
   FROM procurement_orders o JOIN procurement_vendors v ON v.id = o.vendor_id \
   LEFT JOIN inventory_locations l ON l.id = o.ship_to \
   LEFT JOIN users c ON c.id = o.created_by LEFT JOIN users a ON a.id = o.approved_by";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub status: Option<Status>,
    pub vendor_id: Option<Uuid>,
    /// Only orders still to arrive (approved, ordered or partly received).
    #[serde(default)]
    pub open: bool,
    /// `PO-12`, a number, or the vendor's name.
    pub q: Option<String>,
}

/// Orders (without lines), newest first.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> InventoryResult<Vec<Order>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let number: Option<i64> = q.and_then(|q| {
        q.trim_start_matches(['P', 'p'])
            .trim_start_matches(['O', 'o'])
            .trim_start_matches('-')
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
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR o.status = $1) AND ($2::uuid IS NULL OR o.vendor_id = $2) \
            AND (NOT $3 OR o.status IN ('approved', 'ordered', 'partially_received')) \
            AND ($4::text IS NULL OR v.name ILIKE $4 OR o.number = $5) \
          ORDER BY o.created_at DESC LIMIT 500"
    )))
    .bind(filter.status.map(Status::as_str))
    .bind(filter.vendor_id)
    .bind(filter.open)
    .bind(like)
    .bind(number)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Order> {
    let mut o: Order = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE o.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(InventoryError::NotFound)?;
    o.lines = sqlx::query_as(
        "SELECT x.id, x.item_id, i.tracking, x.sku, x.name, x.quantity, x.unit_cost, \
                round(x.quantity * x.unit_cost, 2) AS total, x.received, x.for_ref_type, x.for_ref_id \
           FROM procurement_order_lines x LEFT JOIN inventory_items i ON i.id = x.item_id \
          WHERE x.order_id = $1 ORDER BY x.position",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(o)
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM procurement_orders WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

// ---- drafting ----

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PurchaseOrderLineInput)]
pub struct LineInput {
    /// A stocked item: its SKU and name fill what's left out.
    #[serde(default)]
    pub item_id: Option<Uuid>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(deserialize_with = "crate::number")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_cost: Option<Decimal>,
    /// Bought for a ticket or project: allocated to it on arrival.
    #[serde(default)]
    pub for_ref_type: Option<RefType>,
    #[serde(default)]
    pub for_ref_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PurchaseOrderInput)]
pub struct OrderInput {
    pub vendor_id: Uuid,
    /// Defaults to the vendor's.
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub expected_on: Option<Date>,
    #[serde(default)]
    pub ship_to: Option<Uuid>,
    #[serde(default)]
    pub vendor_reference: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Every line, in order (replaces them all).
    #[serde(default)]
    pub lines: Vec<LineInput>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

struct Header {
    currency: String,
    reference: Option<String>,
    notes: Option<String>,
}

async fn check(
    conn: &mut PgConnection,
    org_id: Uuid,
    input: &OrderInput,
) -> InventoryResult<Header> {
    let vendor: Option<(Uuid, String, bool)> =
        sqlx::query_as("SELECT org_id, currency, active FROM procurement_vendors WHERE id = $1")
            .bind(input.vendor_id)
            .fetch_optional(&mut *conn)
            .await?;
    let currency = match vendor {
        Some((o, _, false)) if o == org_id => {
            return Err(InventoryError::invalid(
                "vendorId",
                "that vendor is turned off",
            ));
        }
        Some((o, c, true)) if o == org_id => c,
        _ => {
            return Err(InventoryError::invalid(
                "vendorId",
                "not a vendor of this organization",
            ));
        }
    };
    let currency = input
        .currency
        .as_deref()
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| !c.is_empty())
        .unwrap_or(currency);
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(InventoryError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ));
    }
    if let Some(at) = input.ship_to {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM inventory_locations WHERE id = $1 AND org_id = $2 AND active)",
        )
        .bind(at)
        .bind(org_id)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(InventoryError::invalid(
                "shipTo",
                "not a location of this organization",
            ));
        }
    }
    if input.lines.len() > 500 {
        return Err(InventoryError::invalid("lines", "at most 500 lines"));
    }
    Ok(Header {
        currency,
        reference: text("vendorReference", input.vendor_reference.as_deref(), 200)?,
        notes: text("notes", input.notes.as_deref(), 4000)?,
    })
}

async fn set_lines(
    conn: &mut PgConnection,
    id: Uuid,
    org_id: Uuid,
    lines: &[LineInput],
) -> InventoryResult<()> {
    sqlx::query("DELETE FROM procurement_order_lines WHERE order_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for (i, l) in lines.iter().enumerate() {
        let field = |f: &str| format!("lines/{i}/{f}");
        if l.quantity <= Decimal::ZERO {
            return Err(InventoryError::invalid(
                &field("quantity"),
                "must be more than zero",
            ));
        }
        if l.unit_cost.is_some_and(|c| c < Decimal::ZERO) {
            return Err(InventoryError::invalid(
                &field("unitCost"),
                "can't be negative",
            ));
        }
        let (mut sku, mut name, mut cost) = (
            text(&field("sku"), l.sku.as_deref(), 64)?,
            text(&field("name"), l.name.as_deref(), 300)?,
            l.unit_cost,
        );
        if let Some(item) = l.item_id {
            let found: Option<(String, String, Option<Decimal>)> = sqlx::query_as(
                "SELECT i.sku, i.name, p.cost FROM inventory_items i \
                   LEFT JOIN catalog_products p ON p.id = i.product_id \
                  WHERE i.id = $1 AND i.org_id = $2",
            )
            .bind(item)
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
            let Some((s, n, c)) = found else {
                return Err(InventoryError::invalid(
                    &field("itemId"),
                    "not an item this organization stocks",
                ));
            };
            sku = sku.or(Some(s));
            name = name.or(Some(n));
            cost = cost.or(c);
            if l.quantity.fract() != Decimal::ZERO {
                let tracking: Tracking =
                    sqlx::query_scalar("SELECT tracking FROM inventory_items WHERE id = $1")
                        .bind(item)
                        .fetch_one(&mut *conn)
                        .await?;
                if tracking == Tracking::Serial {
                    return Err(InventoryError::invalid(
                        &field("quantity"),
                        "whole units for a serial item",
                    ));
                }
            }
        }
        let name = name.ok_or_else(|| InventoryError::invalid(&field("name"), "required"))?;
        if l.for_ref_type.is_some() != l.for_ref_id.is_some() {
            return Err(InventoryError::invalid(
                &field("forRefId"),
                "name the ticket or project",
            ));
        }
        if l.for_ref_type.is_some() && l.item_id.is_none() {
            return Err(InventoryError::invalid(
                &field("forRefType"),
                "only stocked items are allocated on arrival",
            ));
        }
        if let (Some(kind), Some(rid)) = (l.for_ref_type, l.for_ref_id)
            && stock::ref_org(&mut *conn, kind, rid).await?.is_none()
        {
            return Err(InventoryError::invalid(
                &field("forRefId"),
                "no such ticket or project here",
            ));
        }
        sqlx::query(
            "INSERT INTO procurement_order_lines (id, order_id, org_id, position, item_id, sku, name, quantity, \
                unit_cost, for_ref_type, for_ref_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(huxley_core::new_id())
        .bind(id)
        .bind(org_id)
        .bind(i32::try_from(i).unwrap_or(i32::MAX))
        .bind(l.item_id)
        .bind(&sku)
        .bind(&name)
        .bind(l.quantity)
        .bind(cost.unwrap_or_default())
        .bind(l.for_ref_type)
        .bind(l.for_ref_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &OrderInput,
) -> InventoryResult<Order> {
    let h = check(&mut *conn, org_id, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO procurement_orders (id, org_id, vendor_id, currency, expected_on, ship_to, vendor_reference, \
            notes, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.vendor_id)
    .bind(&h.currency)
    .bind(input.expected_on)
    .bind(input.ship_to)
    .bind(&h.reference)
    .bind(&h.notes)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    set_lines(&mut *conn, id, org_id, &input.lines).await?;
    let o = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("purchase_order.created")
            .org(org_id)
            .target("purchase_order", id)
            .details(json!({ "number": o.number, "vendor": o.vendor_name })),
    )
    .await?;
    Ok(o)
}

/// Locks an order, checking the version read and that it's in one of `allowed`.
async fn lock(
    conn: &mut PgConnection,
    id: Uuid,
    version: Option<i32>,
    allowed: &[Status],
    what: &str,
) -> InventoryResult<Order> {
    let (status, current): (Status, i32) =
        sqlx::query_as("SELECT status, version FROM procurement_orders WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(InventoryError::NotFound)?;
    if version.is_some_and(|v| v != current) {
        return Err(InventoryError::Conflict(
            "it was changed by someone else since you loaded it; reload and try again".into(),
        ));
    }
    if !allowed.contains(&status) {
        return Err(InventoryError::Conflict(format!(
            "it can't be {what} while it's {}",
            status.as_str().replace('_', " ")
        )));
    }
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &OrderInput,
) -> InventoryResult<Order> {
    let current = lock(&mut *conn, id, input.version, &[Status::Draft], "changed").await?;
    let h = check(&mut *conn, current.org_id, input).await?;
    sqlx::query(
        "UPDATE procurement_orders SET vendor_id = $2, currency = $3, expected_on = $4, ship_to = $5, \
            vendor_reference = $6, notes = $7, version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(input.vendor_id)
    .bind(&h.currency)
    .bind(input.expected_on)
    .bind(input.ship_to)
    .bind(&h.reference)
    .bind(&h.notes)
    .execute(&mut *conn)
    .await?;
    set_lines(&mut *conn, id, current.org_id, &input.lines).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("purchase_order.updated")
            .org(current.org_id)
            .target("purchase_order", id)
            .details(json!({ "number": current.number })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a draft.
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> InventoryResult<()> {
    let current = lock(&mut *conn, id, None, &[Status::Draft], "deleted").await?;
    sqlx::query("DELETE FROM procurement_orders WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("purchase_order.deleted")
            .org(current.org_id)
            .target("purchase_order", id)
            .details(json!({ "number": current.number })),
    )
    .await?;
    Ok(())
}

// ---- the way through ----

/// What can happen to an order next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = PurchaseOrderStep)]
pub enum Step {
    /// Draft → approved, or submitted when over the approval threshold.
    Submit,
    /// Submitted → approved (`procurement:approve`).
    Approve,
    /// Submitted → back to draft, saying why (`procurement:approve`).
    Reject,
    /// Approved → ordered (sent to the vendor).
    Order,
    /// Anything not yet received → cancelled.
    Cancel,
    /// Partly received → received: the rest isn't coming.
    Close,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PurchaseOrderStepInput)]
pub struct StepInput {
    pub step: Step,
    pub version: i32,
    /// Why (rejecting), or the vendor's order reference (ordering).
    #[serde(default)]
    pub comment: Option<String>,
}

/// The organization's approval threshold.
pub async fn approval_above(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> InventoryResult<Option<Decimal>> {
    Ok(
        sqlx::query_scalar("SELECT approval_above FROM procurement_settings WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

pub async fn set_approval_above(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    above: Option<Decimal>,
) -> InventoryResult<()> {
    if above.is_some_and(|a| a < Decimal::ZERO) {
        return Err(InventoryError::invalid(
            "approvalAbove",
            "can't be negative",
        ));
    }
    sqlx::query(
        "INSERT INTO procurement_settings (org_id, approval_above) VALUES ($1, $2) \
         ON CONFLICT (org_id) DO UPDATE SET approval_above = $2",
    )
    .bind(org_id)
    .bind(above)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("procurement.settings_updated")
            .org(org_id)
            .details(json!({ "approvalAbove": above.map(|a| a.normalize().to_string()) })),
    )
    .await?;
    Ok(())
}

/// Takes an order a step along. The caller checked `procurement:approve`
/// for approving and rejecting.
pub async fn step(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &StepInput,
) -> InventoryResult<Order> {
    use Status::{Approved, Cancelled, Draft, Ordered, PartiallyReceived, Received, Submitted};
    let comment = text("comment", input.comment.as_deref(), 2000)?;
    let (allowed, what): (&[Status], &str) = match input.step {
        Step::Submit => (&[Draft], "submitted"),
        Step::Approve | Step::Reject => (&[Submitted], "decided"),
        Step::Order => (&[Approved], "ordered"),
        Step::Cancel => (&[Draft, Submitted, Approved, Ordered], "cancelled"),
        Step::Close => (&[PartiallyReceived], "closed"),
    };
    let o = lock(&mut *conn, id, Some(input.version), allowed, what).await?;
    let (next, event, sql) = match input.step {
        Step::Submit => {
            if o.lines.is_empty() {
                return Err(InventoryError::invalid(
                    "lines",
                    "add something to order first",
                ));
            }
            let over = approval_above(&mut *conn, o.org_id)
                .await?
                .is_some_and(|a| o.total > a);
            if over {
                (
                    Submitted,
                    "purchase_order.submitted",
                    "submitted_by = $3, submitted_at = now(), rejection = NULL",
                )
            } else {
                (
                    Approved,
                    "purchase_order.approved",
                    "submitted_by = $3, submitted_at = now(), approved_by = $3, approved_at = now(), rejection = NULL",
                )
            }
        }
        Step::Approve => (
            Approved,
            "purchase_order.approved",
            "approved_by = $3, approved_at = now()",
        ),
        Step::Reject => {
            if comment.is_none() {
                return Err(InventoryError::invalid("comment", "say why"));
            }
            (
                Draft,
                "purchase_order.rejected",
                "rejection = $4, submitted_at = NULL",
            )
        }
        Step::Order => (
            Ordered,
            "purchase_order.ordered",
            "ordered_at = now(), vendor_reference = coalesce($4, vendor_reference)",
        ),
        Step::Cancel => {
            if o.lines.iter().any(|l| l.received > Decimal::ZERO) {
                return Err(InventoryError::Conflict(
                    "some of it has arrived; close it instead".into(),
                ));
            }
            (Cancelled, "purchase_order.cancelled", "closed_at = now()")
        }
        Step::Close => (Received, "purchase_order.received", "closed_at = now()"),
    };
    // Each step's fragment uses some of $3 (who) and $4 (the comment); the
    // trailing no-op conditions reference both, as every bound parameter must be.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE procurement_orders SET status = $2, {sql}, version = version + 1, updated_at = now() WHERE id = $1 \
            AND ($3::uuid IS NULL OR true) AND ($4::text IS NULL OR true)"
    )))
    .bind(id)
    .bind(next)
    .bind(ctx.actor.id())
    .bind(&comment)
    .execute(&mut *conn)
    .await?;
    let after = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(o.org_id)
            .target("purchase_order", id)
            .details(json!({ "number": o.number, "comment": comment })),
    )
    .await?;
    emit(&mut *conn, event, &after, comment.as_deref()).await?;
    Ok(after)
}

async fn emit(
    conn: &mut PgConnection,
    event: &'static str,
    o: &Order,
    comment: Option<&str>,
) -> InventoryResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(o.org_id),
            subject_type: "purchase_order",
            subject_id: o.id,
            payload: json!({
                "purchaseOrderId": o.id, "reference": format!("PO-{}", o.number),
                "vendorId": o.vendor_id, "vendor": o.vendor_name, "status": o.status,
                "currency": o.currency, "total": o.total.normalize().to_string(),
                "comment": comment, "link": format!("/purchasing/{}", o.id),
            }),
        },
    )
    .await?;
    Ok(())
}

// ---- receiving ----

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PurchaseOrderReceiptLine)]
pub struct ReceiptLine {
    pub line_id: Uuid,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub quantity: Option<Decimal>,
    #[serde(default)]
    pub lot: Option<String>,
    #[serde(default)]
    pub serials: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = PurchaseOrderReceiptInput)]
pub struct ReceiptInput {
    /// Where it was put (defaults to the order's ship-to location).
    #[serde(default)]
    pub location_id: Option<Uuid>,
    pub lines: Vec<ReceiptLine>,
    /// A delivery note or packing slip.
    #[serde(default)]
    pub reference: Option<String>,
}

/// Takes in a delivery: stocked lines go into inventory (and to their
/// ticket or project, if bought for one).
pub async fn receive(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ReceiptInput,
) -> InventoryResult<Order> {
    let o = lock(
        &mut *conn,
        id,
        None,
        &[Status::Approved, Status::Ordered, Status::PartiallyReceived],
        "received",
    )
    .await?;
    let location = input.location_id.or(o.ship_to);
    let reference = text("reference", input.reference.as_deref(), 200)?;
    if input.lines.is_empty() {
        return Err(InventoryError::invalid("lines", "say what arrived"));
    }
    for (i, r) in input.lines.iter().enumerate() {
        let line = o.lines.iter().find(|l| l.id == r.line_id).ok_or_else(|| {
            InventoryError::invalid(&format!("lines/{i}/lineId"), "not a line of this order")
        })?;
        let quantity = match (
            r.quantity,
            r.serials.iter().filter(|s| !s.trim().is_empty()).count(),
        ) {
            (Some(q), _) => q,
            (None, n) if n > 0 => Decimal::from(n),
            (None, _) => line.quantity - line.received,
        };
        if quantity <= Decimal::ZERO {
            return Err(InventoryError::invalid(
                &format!("lines/{i}/quantity"),
                "must be more than zero",
            ));
        }
        if line.received + quantity > line.quantity {
            return Err(InventoryError::invalid(
                &format!("lines/{i}/quantity"),
                format!(
                    "only {} of {} still to come",
                    (line.quantity - line.received).normalize(),
                    line.name
                ),
            ));
        }
        if let Some(item) = line.item_id {
            let at = location
                .ok_or_else(|| InventoryError::invalid("locationId", "say where it was put"))?;
            let label = reference
                .clone()
                .unwrap_or_else(|| format!("PO-{}", o.number));
            stock_in(&mut *conn, ctx, &o, line, item, at, r, quantity, label).await?;
        }
        sqlx::query("UPDATE procurement_order_lines SET received = received + $2 WHERE id = $1")
            .bind(line.id)
            .bind(quantity)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO procurement_receipts (id, order_id, line_id, org_id, quantity, location_id, lot, serials, \
                received_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(huxley_core::new_id())
        .bind(o.id)
        .bind(line.id)
        .bind(o.org_id)
        .bind(quantity)
        .bind(location)
        .bind(r.lot.as_deref().map(str::trim).unwrap_or_default())
        .bind(r.serials.iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect::<Vec<_>>())
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
    }
    finish_receipt(conn, ctx, &o, input.lines.len()).await
}

/// Puts a received line's stock into inventory, allocating it to the
/// ticket or project it was bought for.
#[allow(clippy::too_many_arguments)]
async fn stock_in(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    o: &Order,
    line: &Line,
    item: Uuid,
    at: Uuid,
    r: &ReceiptLine,
    quantity: Decimal,
    reference: String,
) -> InventoryResult<()> {
    stock::movement_for(
        &mut *conn,
        ctx,
        &MovementInput {
            kind: MovementKind::Receive,
            item_id: item,
            location_id: at,
            to_location_id: None,
            quantity: Some(quantity),
            lot: r.lot.clone(),
            serials: r.serials.clone(),
            unit_cost: Some(line.unit_cost),
            reference: Some(reference),
            note: None,
        },
        Some(("purchase_order", o.id)),
    )
    .await?;
    if let (Some(kind), Some(rid)) = (line.for_ref_type, line.for_ref_id)
        && let Some(ref_org) = stock::ref_org(&mut *conn, kind, rid).await?
    {
        stock::allocate(
            conn,
            ctx,
            &AllocationInput {
                item_id: item,
                location_id: at,
                quantity: Some(quantity),
                lot: r.lot.clone(),
                serials: r.serials.clone(),
                ref_type: kind,
                ref_id: rid,
                note: Some(format!("Bought on PO-{}", o.number)),
                consume: false,
            },
            ref_org,
        )
        .await?;
    }
    Ok(())
}

/// After a delivery: received in full, or in part.
async fn finish_receipt(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    o: &Order,
    lines: usize,
) -> InventoryResult<Order> {
    let id = o.id;
    let all: bool = sqlx::query_scalar(
        "SELECT bool_and(received >= quantity) FROM procurement_order_lines WHERE order_id = $1",
    )
    .bind(o.id)
    .fetch_one(&mut *conn)
    .await?;
    let next = if all {
        Status::Received
    } else {
        Status::PartiallyReceived
    };
    sqlx::query(
        "UPDATE procurement_orders SET status = $2, closed_at = CASE WHEN $3 THEN now() END, \
            version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(o.id)
    .bind(next)
    .bind(all)
    .execute(&mut *conn)
    .await?;
    let after = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("purchase_order.receipt")
            .org(o.org_id)
            .target("purchase_order", id)
            .details(json!({ "number": o.number, "lines": lines, "complete": all })),
    )
    .await?;
    emit(
        &mut *conn,
        if all {
            "purchase_order.received"
        } else {
            "purchase_order.partially_received"
        },
        &after,
        None,
    )
    .await?;
    Ok(after)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PurchaseOrderReceipt)]
pub struct Receipt {
    pub id: Uuid,
    pub line_id: Uuid,
    pub name: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub location_name: Option<String>,
    pub lot: String,
    pub serials: Vec<String>,
    pub received_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
}

pub async fn receipts(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Vec<Receipt>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.line_id, x.name, r.quantity, l.name AS location_name, r.lot, r.serials, \
                u.display_name AS received_by_name, r.received_at \
           FROM procurement_receipts r JOIN procurement_order_lines x ON x.id = r.line_id \
           LEFT JOIN inventory_locations l ON l.id = r.location_id LEFT JOIN users u ON u.id = r.received_by \
          WHERE r.order_id = $1 ORDER BY r.received_at DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

// ---- reordering ----

/// Something running low, and what to order.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ReorderSuggestion)]
pub struct Suggestion {
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub location_id: Uuid,
    pub location_name: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub available: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub reorder_point: Decimal,
    /// Already on open orders.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub on_order: Decimal,
    /// What to order: the reorder quantity (or back above the point), less
    /// what's on order.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    /// The vendor whose name matches the catalog product's.
    pub vendor_id: Option<Uuid>,
    pub vendor_name: Option<String>,
}

/// What's at or under its reorder point at `org_id`'s locations.
pub async fn suggestions(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> InventoryResult<Vec<Suggestion>> {
    Ok(sqlx::query_as(
        "SELECT * FROM ( \
           SELECT i.id AS item_id, i.sku, i.name, l.id AS location_id, l.name AS location_name, \
                  coalesce(s.available, 0) AS available, v.reorder_point, coalesce(oo.qty, 0) AS on_order, \
                  greatest(CASE WHEN v.reorder_qty > 0 THEN v.reorder_qty \
                                ELSE v.reorder_point - coalesce(s.available, 0) + 1 END \
                           - coalesce(oo.qty, 0), 0) AS quantity, \
                  pv.id AS vendor_id, pv.name AS vendor_name \
             FROM inventory_levels v JOIN inventory_items i ON i.id = v.item_id \
             JOIN inventory_locations l ON l.id = v.location_id \
             LEFT JOIN (SELECT item_id, location_id, sum(on_hand - allocated) AS available \
                          FROM inventory_stock GROUP BY item_id, location_id) s \
                    ON s.item_id = v.item_id AND s.location_id = v.location_id \
             LEFT JOIN (SELECT x.item_id, sum(x.quantity - x.received) AS qty \
                          FROM procurement_order_lines x JOIN procurement_orders o ON o.id = x.order_id \
                         WHERE o.status IN ('draft', 'submitted', 'approved', 'ordered', 'partially_received') \
                         GROUP BY x.item_id) oo ON oo.item_id = v.item_id \
             LEFT JOIN catalog_products p ON p.id = i.product_id \
             LEFT JOIN procurement_vendors pv ON pv.org_id = i.org_id AND pv.active \
                    AND lower(pv.name) = lower(p.vendor) \
            WHERE v.low_since IS NOT NULL AND v.org_id = $1) x \
          WHERE x.quantity > 0 ORDER BY x.vendor_name NULLS LAST, x.name",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

/// Drafts an order per vendor from the suggestions that have one (each
/// shipped to the location running low). Returns the orders made.
pub async fn draft_reorders(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
) -> InventoryResult<Vec<Order>> {
    let all = suggestions(&mut *conn, org_id).await?;
    let mut groups: Vec<(Uuid, Uuid, Vec<&Suggestion>)> = Vec::new();
    for s in all.iter().filter(|s| s.vendor_id.is_some()) {
        let vendor = s.vendor_id.unwrap_or_default();
        match groups
            .iter_mut()
            .find(|(v, l, _)| *v == vendor && *l == s.location_id)
        {
            Some(g) => g.2.push(s),
            None => groups.push((vendor, s.location_id, vec![s])),
        }
    }
    let mut made = Vec::new();
    for (vendor, location, lines) in groups {
        let input = OrderInput {
            vendor_id: vendor,
            currency: None,
            expected_on: None,
            ship_to: Some(location),
            vendor_reference: None,
            notes: Some("Reorder of low stock.".into()),
            lines: lines
                .iter()
                .map(|s| LineInput {
                    item_id: Some(s.item_id),
                    sku: None,
                    name: None,
                    quantity: s.quantity.ceil(),
                    unit_cost: None,
                    for_ref_type: None,
                    for_ref_id: None,
                })
                .collect(),
            version: None,
        };
        made.push(create(&mut *conn, ctx, org_id, &input).await?);
    }
    Ok(made)
}
