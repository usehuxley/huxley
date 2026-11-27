//! Quotes: what's offered to a client, line by line.
//!
//! - Lines come from the catalog (or are written in), each one-time or
//!   recurring (monthly, quarterly, annual), with a quantity, cost, price
//!   and discount. Headings group them. **Options** count only when
//!   selected, and the client may choose them when accepting.
//! - **Totals:** one-time, recurring per month (quarterly ÷ 3, annual ÷ 12),
//!   cost, and margin on the first year (one-time + 12 months).
//! - **Revisions:** sending keeps a snapshot of what was sent. Changing a
//!   sent quote starts the next revision (back to draft).
//! - **Deciding:** a sent quote is accepted (with the options taken) or
//!   declined. Accepting wins its opportunity if it's still open.
//! - Raises `sales.quote.created`, `.sent`, `.accepted` and `.declined`.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::opportunities::{self, MoveInput};
use crate::stages::{self, StageKind};
use crate::{SalesError, SalesResult, currency, money, plain, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = QuoteStatus)]
pub enum Status {
    Draft,
    Sent,
    Accepted,
    Declined,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = QuoteLineKind)]
pub enum LineKind {
    #[default]
    Item,
    Heading,
}

/// How often a line is charged.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = QuoteBilling)]
pub enum Billing {
    #[default]
    OneTime,
    Monthly,
    Quarterly,
    Annual,
}

impl Billing {
    /// What one charge is worth per month (`None` for one-time).
    fn per_month(self, amount: Decimal) -> Option<Decimal> {
        match self {
            Self::OneTime => None,
            Self::Monthly => Some(amount),
            Self::Quarterly => Some(amount / Decimal::from(3)),
            Self::Annual => Some(amount / Decimal::from(12)),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = QuoteLine)]
pub struct Line {
    pub id: Uuid,
    pub position: i32,
    pub kind: LineKind,
    pub product_id: Option<Uuid>,
    pub sku: Option<String>,
    pub name: String,
    pub description: String,
    pub billing: Billing,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub unit: String,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub unit_cost: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    /// Percent off, 0 to 100.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub discount_pct: Decimal,
    pub taxable: bool,
    /// An option the client may take or leave.
    pub optional: bool,
    /// Counted in the totals (always, unless an option left out).
    pub selected: bool,
    /// Quantity × price, less the discount (per charge).
    #[sqlx(skip)]
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = QuoteTotals)]
pub struct Totals {
    /// One-time lines.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub one_time: Decimal,
    /// Recurring lines, per month.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub monthly: Decimal,
    /// One-time + 12 months of recurring.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub first_year: Decimal,
    /// The cost of the first year.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub first_year_cost: Decimal,
    /// Margin on the first year, as a percent (none without revenue).
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub margin_pct: Option<Decimal>,
    /// What the discounts take off the first year.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = String)]
    pub discounts: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    /// `Q-` and this number.
    pub number: i64,
    pub opportunity_id: Option<Uuid>,
    pub opportunity_name: Option<String>,
    pub title: String,
    pub status: Status,
    pub revision: i32,
    pub currency: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub valid_until: Option<Date>,
    pub contact_id: Option<Uuid>,
    pub contact_name: Option<String>,
    pub contact_email: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub intro: String,
    pub terms: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub sent_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub decided_by: Option<String>,
    pub decline_reason: Option<String>,
    /// Whether it needs (or had) an internal review.
    pub review_status: crate::review::ReviewStatus,
    /// The sales order it became, once accepted.
    pub sales_order_id: Option<Uuid>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[sqlx(skip)]
    pub lines: Vec<Line>,
    #[sqlx(skip)]
    pub totals: Totals,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = QuoteLineInput)]
pub struct LineInput {
    /// An existing line's id, to keep it (and its comments); new lines leave it out.
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub kind: LineKind,
    /// From the catalog; its SKU, name, unit, cost and price fill what's left out.
    #[serde(default)]
    pub product_id: Option<Uuid>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub billing: Option<Billing>,
    #[serde(default, deserialize_with = "crate::money_opt")]
    #[schema(value_type = Option<String>)]
    pub quantity: Option<Decimal>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default, deserialize_with = "crate::money_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_cost: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::money_opt")]
    #[schema(value_type = Option<String>)]
    pub unit_price: Option<Decimal>,
    #[serde(default, deserialize_with = "money")]
    #[schema(value_type = Option<String>)]
    pub discount_pct: Decimal,
    #[serde(default)]
    pub taxable: Option<bool>,
    #[serde(default)]
    pub optional: bool,
    /// Whether an option is taken (default no: options count once the client takes them).
    #[serde(default)]
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteInput {
    pub title: String,
    #[serde(default)]
    pub opportunity_id: Option<Uuid>,
    #[serde(default = "usd")]
    pub currency: String,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub valid_until: Option<Date>,
    #[serde(default)]
    pub contact_id: Option<Uuid>,
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub terms: String,
    /// Every line, in order (replaces them all).
    #[serde(default)]
    pub lines: Vec<LineInput>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

fn usd() -> String {
    "USD".into()
}

/// The client's answer.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionInput {
    pub accept: bool,
    /// Who decided, as they'd have it written.
    pub name: String,
    /// Why, when declining.
    #[serde(default)]
    pub reason: Option<String>,
    /// The options taken (accepting only); others are left out.
    #[serde(default)]
    pub options: Vec<Uuid>,
    pub version: i32,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub org_id: Option<Uuid>,
    pub opportunity_id: Option<Uuid>,
    pub status: Option<Status>,
    /// In the title, the client's name or the number.
    pub q: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub revision: i32,
    pub sent_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub sent_at: OffsetDateTime,
    /// The quote as sent, lines and totals included.
    #[schema(value_type = Object)]
    pub snapshot: Json,
}

const SELECT: &str = "SELECT q.id, q.org_id, g.name AS org_name, q.number, q.opportunity_id, \
        o.name AS opportunity_name, q.title, q.status, q.revision, q.currency, q.valid_until, \
        q.contact_id, nullif(btrim(coalesce(c.first_name, '') || ' ' || coalesce(c.last_name, '')), '') AS contact_name, \
        c.email AS contact_email, q.owner_id, u.display_name AS owner_name, q.intro, q.terms, \
        q.sent_at, q.decided_at, q.decided_by, q.decline_reason, q.review_status, \
        (SELECT so.id FROM sales_orders so WHERE so.quote_id = q.id) AS sales_order_id, q.version, \
        q.created_at, q.updated_at \
   FROM sales_quotes q \
   JOIN organizations g ON g.id = q.org_id \
   LEFT JOIN sales_opportunities o ON o.id = q.opportunity_id \
   LEFT JOIN crm_contacts c ON c.id = q.contact_id \
   LEFT JOIN users u ON u.id = q.owner_id";

fn round2(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// A line's total: quantity × price, less the discount.
fn line_total(quantity: Decimal, price: Decimal, discount: Decimal) -> Decimal {
    round2(quantity * price * (Decimal::ONE_HUNDRED - discount) / Decimal::ONE_HUNDRED)
}

/// The totals of the lines that count.
pub fn totals(lines: &[Line]) -> Totals {
    let mut t = Totals::default();
    let (mut monthly_cost, mut one_time_cost) = (Decimal::ZERO, Decimal::ZERO);
    let (mut monthly_list, mut one_time_list) = (Decimal::ZERO, Decimal::ZERO);
    for l in lines
        .iter()
        .filter(|l| l.kind == LineKind::Item && (!l.optional || l.selected))
    {
        let cost = l.quantity * l.unit_cost;
        let list = l.quantity * l.unit_price;
        match l.billing.per_month(l.total) {
            None => {
                t.one_time += l.total;
                one_time_cost += cost;
                one_time_list += list;
            }
            Some(per_month) => {
                t.monthly += per_month;
                monthly_cost += l.billing.per_month(cost).unwrap_or_default();
                monthly_list += l.billing.per_month(list).unwrap_or_default();
            }
        }
    }
    let twelve = Decimal::from(12);
    t.one_time = round2(t.one_time);
    t.monthly = round2(t.monthly);
    t.first_year = round2(t.one_time + twelve * t.monthly);
    t.first_year_cost = round2(one_time_cost + twelve * monthly_cost);
    t.discounts = round2(one_time_list + twelve * monthly_list - t.first_year).max(Decimal::ZERO);
    t.margin_pct = (t.first_year > Decimal::ZERO)
        .then(|| (t.first_year - t.first_year_cost) / t.first_year * Decimal::ONE_HUNDRED);
    t.margin_pct = t.margin_pct.map(|m| m.round_dp(1));
    t
}

async fn lines_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Vec<Line>> {
    let mut lines: Vec<Line> = sqlx::query_as(
        "SELECT id, position, kind, product_id, sku, name, description, billing, quantity, unit, \
                unit_cost, unit_price, discount_pct, taxable, optional, selected \
           FROM sales_quote_lines WHERE quote_id = $1 ORDER BY position",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    for l in &mut lines {
        l.total = line_total(l.quantity, l.unit_price, l.discount_pct);
    }
    Ok(lines)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Quote> {
    let mut q: Quote = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE q.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(SalesError::NotFound)?;
    q.lines = lines_of(conn, id).await?;
    q.totals = totals(&q.lines);
    Ok(q)
}

/// Quotes you can see, newest first (at most 500), with their totals.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> SalesResult<Vec<Quote>> {
    let q = filter
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| q.trim_start_matches(['Q', 'q', '-']).to_owned());
    let mut quotes: Vec<Quote> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR q.org_id = $1) \
            AND ($2::uuid IS NULL OR q.opportunity_id = $2) \
            AND ($3::text IS NULL OR q.status = $3) \
            AND ($4::text IS NULL OR q.title ILIKE '%' || $4 || '%' OR g.name ILIKE '%' || $4 || '%' \
                 OR q.number::text = $4) \
          ORDER BY q.updated_at DESC LIMIT 500"
    )))
    .bind(filter.org_id)
    .bind(filter.opportunity_id)
    .bind(filter.status)
    .bind(q)
    .fetch_all(&mut *conn)
    .await?;
    for quote in &mut quotes {
        quote.lines = lines_of(&mut *conn, quote.id).await?;
        quote.totals = totals(&quote.lines);
    }
    Ok(quotes)
}

/// A catalog product's defaults for a line.
#[derive(sqlx::FromRow)]
struct ProductDefaults {
    sku: String,
    name: String,
    description: String,
    unit: String,
    cost: Option<Decimal>,
    price: Option<Decimal>,
    billing: String,
    taxable: bool,
}

/// A line ready to store, its blanks filled from the catalog.
struct Ready {
    kind: LineKind,
    product_id: Option<Uuid>,
    sku: Option<String>,
    name: String,
    description: String,
    billing: Billing,
    quantity: Decimal,
    unit: String,
    unit_cost: Decimal,
    unit_price: Decimal,
    discount_pct: Decimal,
    taxable: bool,
    optional: bool,
    selected: bool,
}

async fn ready(conn: &mut PgConnection, i: usize, l: &LineInput) -> SalesResult<Ready> {
    let field = |f: &str| format!("lines/{i}/{f}");
    let product: Option<ProductDefaults> = match l.product_id {
        Some(p) => Some(
            sqlx::query_as(
                "SELECT sku, name, description, unit, cost, \
                        coalesce(price, (SELECT sum(c.price * b.quantity) FROM catalog_bundle_items b \
                                          JOIN catalog_products c ON c.id = b.product_id WHERE b.bundle_id = p.id)) AS price, \
                        billing, taxable \
                   FROM catalog_products p WHERE id = $1",
            )
            .bind(p)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| SalesError::invalid(&field("productId"), "not in the catalog"))?,
        ),
        None => None,
    };
    let billing = l
        .billing
        .unwrap_or_else(|| match product.as_ref().map(|p| p.billing.as_str()) {
            Some("monthly") => Billing::Monthly,
            Some("quarterly") => Billing::Quarterly,
            Some("annual") => Billing::Annual,
            _ => Billing::OneTime,
        });
    let name = l
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| product.as_ref().map(|p| p.name.clone()))
        .ok_or_else(|| SalesError::invalid(&field("name"), "required"))?;
    let quantity = l.quantity.unwrap_or(Decimal::ONE);
    let unit_cost = l
        .unit_cost
        .or_else(|| product.as_ref().and_then(|p| p.cost))
        .unwrap_or_default();
    let unit_price = l
        .unit_price
        .or_else(|| product.as_ref().and_then(|p| p.price))
        .unwrap_or_default();
    for (f, v) in [
        ("quantity", quantity),
        ("unitCost", unit_cost),
        ("unitPrice", unit_price),
    ] {
        if v < Decimal::ZERO {
            return Err(SalesError::invalid(&field(f), "can't be negative"));
        }
    }
    if !(Decimal::ZERO..=Decimal::ONE_HUNDRED).contains(&l.discount_pct) {
        return Err(SalesError::invalid(&field("discountPct"), "0 to 100"));
    }
    let description = l
        .description
        .clone()
        .or_else(|| product.as_ref().map(|p| p.description.clone()))
        .unwrap_or_default();
    if description.chars().count() > 4000 {
        return Err(SalesError::invalid(
            &field("description"),
            "at most 4000 characters",
        ));
    }
    Ok(Ready {
        kind: l.kind,
        product_id: l.product_id,
        sku: text(&field("sku"), l.sku.as_deref(), 64)?
            .or_else(|| product.as_ref().map(|p| p.sku.clone())),
        name: required(&field("name"), &name, 300)?,
        description,
        billing,
        quantity,
        unit: text(&field("unit"), l.unit.as_deref(), 32)?
            .or_else(|| product.as_ref().map(|p| p.unit.clone()))
            .unwrap_or_else(|| "each".into()),
        unit_cost,
        unit_price,
        discount_pct: l.discount_pct,
        taxable: l
            .taxable
            .or_else(|| product.as_ref().map(|p| p.taxable))
            .unwrap_or(true),
        optional: l.optional,
        selected: !l.optional || l.selected.unwrap_or(false),
    })
}

async fn set_lines(
    conn: &mut PgConnection,
    id: Uuid,
    org_id: Uuid,
    lines: &[LineInput],
) -> SalesResult<()> {
    if lines.len() > 500 {
        return Err(SalesError::invalid("lines", "at most 500 lines"));
    }
    let mut ready_lines = Vec::with_capacity(lines.len());
    for (i, l) in lines.iter().enumerate() {
        ready_lines.push(ready(&mut *conn, i, l).await?);
    }
    // Lines named by id keep it; the rest are new, and lines not named go.
    let existing: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM sales_quote_lines WHERE quote_id = $1")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    let mut keep = Vec::new();
    for l in lines {
        if let Some(line) =
            l.id.filter(|line| existing.contains(line) && !keep.contains(line))
        {
            keep.push(line);
        }
    }
    sqlx::query("DELETE FROM sales_quote_lines WHERE quote_id = $1 AND NOT (id = ANY($2))")
        .bind(id)
        .bind(&keep)
        .execute(&mut *conn)
        .await?;
    let mut used = Vec::new();
    for (i, (input, l)) in lines.iter().zip(ready_lines).enumerate() {
        let line_id = match input
            .id
            .filter(|line| keep.contains(line) && !used.contains(line))
        {
            Some(line) => line,
            None => huxley_core::new_id(),
        };
        used.push(line_id);
        sqlx::query(
            "INSERT INTO sales_quote_lines (id, quote_id, org_id, position, kind, product_id, sku, name, \
                description, billing, quantity, unit, unit_cost, unit_price, discount_pct, taxable, \
                optional, selected) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) \
             ON CONFLICT (id) DO UPDATE SET position = EXCLUDED.position, kind = EXCLUDED.kind, \
                product_id = EXCLUDED.product_id, sku = EXCLUDED.sku, name = EXCLUDED.name, \
                description = EXCLUDED.description, billing = EXCLUDED.billing, \
                quantity = EXCLUDED.quantity, unit = EXCLUDED.unit, unit_cost = EXCLUDED.unit_cost, \
                unit_price = EXCLUDED.unit_price, discount_pct = EXCLUDED.discount_pct, \
                taxable = EXCLUDED.taxable, optional = EXCLUDED.optional, selected = EXCLUDED.selected",
        )
        .bind(line_id)
        .bind(id)
        .bind(org_id)
        .bind(i32::try_from(i).unwrap_or(i32::MAX))
        .bind(l.kind)
        .bind(l.product_id)
        .bind(&l.sku)
        .bind(&l.name)
        .bind(&l.description)
        .bind(l.billing)
        .bind(l.quantity)
        .bind(&l.unit)
        .bind(l.unit_cost)
        .bind(l.unit_price)
        .bind(l.discount_pct)
        .bind(l.taxable)
        .bind(l.optional)
        .bind(l.selected)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

struct Header {
    title: String,
    currency: String,
}

async fn check(conn: &mut PgConnection, org_id: Uuid, input: &QuoteInput) -> SalesResult<Header> {
    for (field, value) in [("intro", &input.intro), ("terms", &input.terms)] {
        if value.chars().count() > 20000 {
            return Err(SalesError::invalid(field, "at most 20000 characters"));
        }
    }
    if let Some(opportunity) = input.opportunity_id {
        let org: Option<Uuid> = opportunities::org_of(&mut *conn, opportunity).await?;
        if org != Some(org_id) {
            return Err(SalesError::invalid(
                "opportunityId",
                "not an opportunity of this client",
            ));
        }
    }
    if let Some(contact) = input.contact_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM crm_contact_companies WHERE contact_id = $1 AND org_id = $2)",
        )
        .bind(contact)
        .bind(org_id)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(SalesError::invalid(
                "contactId",
                "not a contact of this client",
            ));
        }
    }
    Ok(Header {
        title: required("title", &input.title, 200)?,
        currency: currency(&input.currency)?,
    })
}

async fn emit(conn: &mut PgConnection, event: &'static str, q: &Quote) -> SalesResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(q.org_id),
            subject_type: "quote",
            subject_id: q.id,
            payload: json!({
                "quoteId": q.id,
                "number": q.number,
                "title": q.title,
                "orgId": q.org_id,
                "opportunityId": q.opportunity_id,
                "status": q.status,
                "revision": q.revision,
                "currency": q.currency,
                "oneTime": q.totals.one_time.normalize().to_string(),
                "monthly": q.totals.monthly.normalize().to_string(),
                "marginPct": q.totals.margin_pct.map(|m| m.normalize().to_string()),
                "ownerId": q.owner_id,
                "contactId": q.contact_id,
            }),
        },
    )
    .await?;
    Ok(())
}

/// Creates a draft quote for the client `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &QuoteInput,
) -> SalesResult<Quote> {
    let h = check(&mut *conn, org_id, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_quotes (id, org_id, opportunity_id, title, currency, valid_until, contact_id, \
            owner_id, intro, terms, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.opportunity_id)
    .bind(&h.title)
    .bind(&h.currency)
    .bind(input.valid_until)
    .bind(input.contact_id)
    .bind(input.owner_id.or_else(|| ctx.actor.id()))
    .bind(&input.intro)
    .bind(&input.terms)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    set_lines(&mut *conn, id, org_id, &input.lines).await?;
    crate::review::refresh(&mut *conn, id, false).await?;
    let q = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.created")
            .org(org_id)
            .target("quote", id)
            .details(json!({ "title": q.title, "number": q.number })),
    )
    .await?;
    emit(&mut *conn, "sales.quote.created", &q).await?;
    Ok(q)
}

/// Saves the quote and its lines. A sent quote goes back to draft as its
/// next revision; an accepted or declined one can't change.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &QuoteInput,
) -> SalesResult<Quote> {
    let current = get(&mut *conn, id).await?;
    let version = input
        .version
        .ok_or_else(|| SalesError::invalid("version", "send the version you read"))?;
    if current.version != version {
        return Err(SalesError::VersionConflict);
    }
    if matches!(current.status, Status::Accepted | Status::Declined) {
        return Err(SalesError::Conflict(
            "it's been decided; copy it to quote again".into(),
        ));
    }
    let h = check(&mut *conn, current.org_id, input).await?;
    let next_revision = current.status == Status::Sent;
    sqlx::query(
        "UPDATE sales_quotes SET opportunity_id = $2, title = $3, currency = $4, valid_until = $5, \
            contact_id = $6, owner_id = $7, intro = $8, terms = $9, status = 'draft', \
            revision = revision + $10, version = version + 1, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(input.opportunity_id)
    .bind(&h.title)
    .bind(&h.currency)
    .bind(input.valid_until)
    .bind(input.contact_id)
    .bind(input.owner_id)
    .bind(&input.intro)
    .bind(&input.terms)
    .bind(i32::from(next_revision))
    .execute(&mut *conn)
    .await?;
    set_lines(&mut *conn, id, current.org_id, &input.lines).await?;
    crate::review::refresh(&mut *conn, id, true).await?;
    let q = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.updated")
            .org(q.org_id)
            .target("quote", id)
            .details(json!({
                "revision": q.revision,
                "before": { "oneTime": current.totals.one_time.normalize().to_string(),
                            "monthly": current.totals.monthly.normalize().to_string() },
                "after": { "oneTime": q.totals.one_time.normalize().to_string(),
                           "monthly": q.totals.monthly.normalize().to_string() },
            })),
    )
    .await?;
    Ok(q)
}

/// Marks it sent and keeps what was sent as its revision.
pub async fn send(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
    override_review: bool,
) -> SalesResult<Quote> {
    let current = get(&mut *conn, id).await?;
    if current.version != version {
        return Err(SalesError::VersionConflict);
    }
    if current.status != Status::Draft {
        return Err(SalesError::Conflict(match current.status {
            Status::Sent => "it's already sent; change it to send a new revision".into(),
            _ => "it's been decided".into(),
        }));
    }
    if !current.lines.iter().any(|l| l.kind == LineKind::Item) {
        return Err(SalesError::invalid("lines", "add something to quote first"));
    }
    crate::review::refresh(&mut *conn, id, false).await?;
    if !crate::review::may_send(&mut *conn, &current).await? {
        if !override_review {
            return Err(SalesError::Conflict(
                "it needs an approved review before it's sent".into(),
            ));
        }
        crate::review::record_override(&mut *conn, ctx, &current).await?;
    }
    sqlx::query(
        "UPDATE sales_quotes SET status = 'sent', sent_at = now(), version = version + 1, \
            updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let q = get(&mut *conn, id).await?;
    sqlx::query(
        "INSERT INTO sales_quote_revisions (id, quote_id, org_id, revision, snapshot, sent_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (quote_id, revision) DO UPDATE SET snapshot = EXCLUDED.snapshot, \
            sent_by = EXCLUDED.sent_by, sent_at = now()",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(q.org_id)
    .bind(q.revision)
    .bind(serde_json::to_value(&q).unwrap_or(Json::Null))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("quote.sent")
            .org(q.org_id)
            .target("quote", id)
            .details(json!({ "revision": q.revision, "contactId": q.contact_id })),
    )
    .await?;
    emit(&mut *conn, "sales.quote.sent", &q).await?;
    Ok(q)
}

/// Records the client's answer to a sent quote. Accepting takes the options
/// named (others left out) and wins its open opportunity.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &DecisionInput,
) -> SalesResult<Quote> {
    let current = get(&mut *conn, id).await?;
    if current.version != input.version {
        return Err(SalesError::VersionConflict);
    }
    if current.status != Status::Sent {
        return Err(SalesError::Conflict(match current.status {
            Status::Draft => "it hasn't been sent".into(),
            _ => "it's already been decided".into(),
        }));
    }
    let name = required("name", &input.name, 200)?;
    let reason = text("reason", input.reason.as_deref(), 2000)?;
    if input.accept {
        let options: Vec<Uuid> = current
            .lines
            .iter()
            .filter(|l| l.optional)
            .map(|l| l.id)
            .collect();
        if let Some(bad) = input.options.iter().find(|o| !options.contains(o)) {
            return Err(SalesError::invalid(
                "options",
                format!("{bad} isn't an option on it"),
            ));
        }
        sqlx::query(
            "UPDATE sales_quote_lines SET selected = (id = ANY($2)) WHERE quote_id = $1 AND optional",
        )
        .bind(id)
        .bind(&input.options)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE sales_quotes SET status = $2, decided_at = now(), decided_by = $3, decline_reason = $4, \
            version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(if input.accept { Status::Accepted } else { Status::Declined })
    .bind(&name)
    .bind(if input.accept { None } else { reason })
    .execute(&mut *conn)
    .await?;
    if input.accept {
        win_opportunity(&mut *conn, ctx, current.opportunity_id).await?;
        crate::orders::from_quote(&mut *conn, ctx, id).await?;
    }
    let q = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if input.accept {
            "quote.accepted"
        } else {
            "quote.declined"
        })
        .org(q.org_id)
        .target("quote", id)
        .details(json!({ "by": name, "revision": q.revision, "reason": q.decline_reason })),
    )
    .await?;
    emit(
        &mut *conn,
        if input.accept {
            "sales.quote.accepted"
        } else {
            "sales.quote.declined"
        },
        &q,
    )
    .await?;
    Ok(q)
}

/// Moves the quote's opportunity to the first won stage, if it's open.
async fn win_opportunity(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    opportunity: Option<Uuid>,
) -> SalesResult<()> {
    let Some(opportunity) = opportunity else {
        return Ok(());
    };
    let o = opportunities::get(&mut *conn, opportunity).await?;
    if o.status != opportunities::Status::Open {
        return Ok(());
    }
    let won = stages::first_of(&mut *conn, StageKind::Won).await?;
    opportunities::move_to(
        conn,
        ctx,
        opportunity,
        &MoveInput {
            stage_id: won.id,
            reason_id: None,
            reason_note: Some("Quote accepted".into()),
            version: o.version,
        },
    )
    .await?;
    Ok(())
}

/// What was sent, newest first.
pub async fn revisions(conn: &mut PgConnection, id: Uuid) -> SalesResult<Vec<Revision>> {
    Ok(sqlx::query_as(
        "SELECT r.revision, u.display_name AS sent_by_name, r.sent_at, r.snapshot \
           FROM sales_quote_revisions r LEFT JOIN users u ON u.id = r.sent_by \
          WHERE r.quote_id = $1 ORDER BY r.revision DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Deletes a quote that was never sent.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<()> {
    let q = get(&mut *conn, id).await?;
    let sent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sales_quote_revisions WHERE quote_id = $1)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if sent {
        return Err(SalesError::Conflict(
            "it's been sent, so it stays on record".into(),
        ));
    }
    sqlx::query("DELETE FROM sales_quotes WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("quote.deleted")
            .org(q.org_id)
            .target("quote", id)
            .details(json!({ "title": q.title, "number": q.number })),
    )
    .await?;
    Ok(())
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_quotes WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(billing: Billing, qty: i64, cost: i64, price: i64, discount: i64) -> Line {
        let (quantity, unit_price, discount_pct) = (
            Decimal::from(qty),
            Decimal::from(price),
            Decimal::from(discount),
        );
        Line {
            id: Uuid::nil(),
            position: 0,
            kind: LineKind::Item,
            product_id: None,
            sku: None,
            name: "x".into(),
            description: String::new(),
            billing,
            quantity,
            unit: "each".into(),
            unit_cost: Decimal::from(cost),
            unit_price,
            discount_pct,
            taxable: true,
            optional: false,
            selected: true,
            total: line_total(quantity, unit_price, discount_pct),
        }
    }

    #[test]
    fn totals_add_one_time_and_monthly_with_first_year_margin() {
        let mut option = line(Billing::OneTime, 1, 50, 100, 0);
        option.optional = true;
        option.selected = false;
        let t = totals(&[
            line(Billing::OneTime, 2, 800, 1000, 10), // 1800, cost 1600, list 2000
            line(Billing::Monthly, 10, 15, 25, 0),    // 250/mo, cost 150
            line(Billing::Annual, 1, 600, 1200, 0),   // 100/mo, cost 50
            option,
        ]);
        assert_eq!(t.one_time, Decimal::from(1800));
        assert_eq!(t.monthly, Decimal::from(350));
        assert_eq!(t.first_year, Decimal::from(1800 + 12 * 350));
        assert_eq!(t.first_year_cost, Decimal::from(1600 + 12 * 200));
        assert_eq!(t.discounts, Decimal::from(200));
        // (6000 - 4000) / 6000 = 33.3%.
        assert_eq!(t.margin_pct, Some(Decimal::new(333, 1)));
    }

    #[test]
    fn no_revenue_has_no_margin() {
        assert_eq!(totals(&[]).margin_pct, None);
        assert_eq!(
            line_total(Decimal::new(15, 1), Decimal::new(999, 2), Decimal::ZERO),
            Decimal::new(1499, 2)
        );
    }
}
