//! Invoices and credit memos: drafts until approved, then sent (or synced),
//! never changed after; corrections are credit memos.

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

use crate::{BillingError, BillingResult, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InvoiceStatus)]
pub enum Status {
    Draft,
    Approved,
    /// Sent to the client or synced to accounting: locked.
    Sent,
    Void,
}

impl Status {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Sent => "sent",
            Self::Void => "void",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InvoiceLineKind)]
pub enum LineKind {
    Time,
    Product,
    Recurring,
    Project,
    Manual,
}

impl LineKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Product => "product",
            Self::Recurring => "recurring",
            Self::Project => "project",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InvoiceLine)]
pub struct Line {
    pub id: Uuid,
    pub kind: LineKind,
    pub description: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    pub unit: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    pub product_id: Option<Uuid>,
    pub tax_code: Option<String>,
    pub edited: bool,
    /// How many sources it bills (time entries, order lines…).
    pub sources: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Invoice {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub number: i64,
    /// `invoice` or `credit_memo`.
    pub kind: String,
    pub credit_of: Option<Uuid>,
    pub status: Status,
    pub on_hold: bool,
    pub hold_reason: Option<String>,
    pub currency: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub period_start: Option<Date>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub period_end: Option<Date>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub issued_on: Option<Date>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_on: Option<Date>,
    pub notes: Option<String>,
    pub run_id: Option<Uuid>,
    pub approved_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub approved_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    /// Whether it needs (or had) an internal review (ADR-076).
    pub review_status: crate::review::ReviewStatus,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[sqlx(skip)]
    pub lines: Vec<Line>,
}

const SELECT: &str = "SELECT i.id, i.org_id, o.name AS org_name, i.number, i.kind, i.credit_of, i.status, i.on_hold, \
        i.hold_reason, i.currency, i.period_start, i.period_end, i.issued_on, i.due_on, i.notes, i.run_id, \
        a.display_name AS approved_by_name, i.approved_at, \
        coalesce((SELECT sum(round(l.quantity * l.unit_price, 2)) FROM billing_invoice_lines l \
                   WHERE l.invoice_id = i.id), 0) AS total, \
        i.review_status, i.version, i.created_at \
   FROM billing_invoices i JOIN organizations o ON o.id = i.org_id LEFT JOIN users a ON a.id = i.approved_by";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub status: Option<Status>,
    pub org_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    /// `INV-12`, a number, or the client's name.
    pub q: Option<String>,
}

pub async fn list(conn: &mut PgConnection, filter: &Filter) -> BillingResult<Vec<Invoice>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let number: Option<i64> = q.and_then(|q| {
        q.to_ascii_uppercase()
            .trim_start_matches("INV-")
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
        "{SELECT} WHERE ($1::text IS NULL OR i.status = $1) AND ($2::uuid IS NULL OR i.org_id = $2) \
            AND ($3::uuid IS NULL OR i.run_id = $3) AND ($4::text IS NULL OR o.name ILIKE $4 OR i.number = $5) \
          ORDER BY i.created_at DESC, i.number DESC LIMIT 500"
    )))
    .bind(filter.status.map(Status::as_str))
    .bind(filter.org_id)
    .bind(filter.run_id)
    .bind(like)
    .bind(number)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> BillingResult<Invoice> {
    let mut i: Invoice = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE i.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(BillingError::NotFound)?;
    i.lines = sqlx::query_as(
        "SELECT l.id, l.kind, l.description, l.quantity, l.unit, l.unit_price, round(l.quantity * l.unit_price, 2) AS amount, \
                l.product_id, l.tax_code, l.edited, (SELECT count(*) FROM billing_billed b WHERE b.line_id = l.id) AS sources \
           FROM billing_invoice_lines l WHERE l.invoice_id = $1 ORDER BY l.position",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(i)
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> BillingResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM billing_invoices WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Locks an invoice, checking the version read and that it's in `allowed`.
async fn lock(
    conn: &mut PgConnection,
    id: Uuid,
    version: Option<i32>,
    allowed: &[Status],
    what: &str,
) -> BillingResult<Invoice> {
    let (status, current): (Status, i32) =
        sqlx::query_as("SELECT status, version FROM billing_invoices WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(BillingError::NotFound)?;
    if version.is_some_and(|v| v != current) {
        return Err(BillingError::Conflict(
            "it was changed by someone else since you loaded it; reload and try again".into(),
        ));
    }
    if !allowed.contains(&status) {
        return Err(BillingError::Conflict(format!(
            "it can't be {what} while it's {}",
            status.as_str()
        )));
    }
    get(conn, id).await
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InvoiceLineInput)]
pub struct LineInput {
    /// An existing line (keeps what it bills); new lines leave it out.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub description: String,
    #[serde(deserialize_with = "crate::number")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(deserialize_with = "crate::number")]
    #[schema(value_type = String)]
    pub unit_price: Decimal,
    #[serde(default)]
    pub product_id: Option<Uuid>,
    #[serde(default)]
    pub tax_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InvoiceInput)]
pub struct InvoiceInput {
    pub lines: Vec<LineInput>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_on: Option<Date>,
    pub version: i32,
}

/// Changes a draft: its lines (a line removed gives back what it billed;
/// changed or added lines are flagged as edited), notes and due date.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &InvoiceInput,
) -> BillingResult<Invoice> {
    let current = lock(
        &mut *conn,
        id,
        Some(input.version),
        &[Status::Draft],
        "changed",
    )
    .await?;
    if input.lines.len() > 500 {
        return Err(BillingError::invalid("lines", "at most 500 lines"));
    }
    let keep: Vec<Uuid> = input
        .lines
        .iter()
        .filter_map(|l| l.id)
        .filter(|l| current.lines.iter().any(|c| c.id == *l))
        .collect();
    // Lines that go give back what they billed (billing_billed cascades).
    sqlx::query("DELETE FROM billing_invoice_lines WHERE invoice_id = $1 AND NOT (id = ANY($2))")
        .bind(id)
        .bind(&keep)
        .execute(&mut *conn)
        .await?;
    for (i, l) in input.lines.iter().enumerate() {
        let f = |k: &str| format!("lines/{i}/{k}");
        let description = crate::required(&f("description"), &l.description, 1000)?;
        let unit = text(&f("unit"), l.unit.as_deref(), 32)?.unwrap_or_else(|| "each".into());
        let tax = text(&f("taxCode"), l.tax_code.as_deref(), 50)?;
        match l
            .id
            .filter(|x| keep.contains(x))
            .and_then(|x| current.lines.iter().find(|c| c.id == x))
        {
            Some(before) => {
                let changed = before.description != description
                    || before.quantity != l.quantity
                    || before.unit_price != l.unit_price
                    || before.unit != unit
                    || before.tax_code != tax;
                sqlx::query(
                    "UPDATE billing_invoice_lines SET position = $2, description = $3, quantity = $4, unit = $5, \
                        unit_price = $6, product_id = $7, tax_code = $8, edited = edited OR $9 WHERE id = $1",
                )
                .bind(before.id)
                .bind(i32::try_from(i).unwrap_or(0))
                .bind(&description)
                .bind(l.quantity)
                .bind(&unit)
                .bind(l.unit_price)
                .bind(l.product_id)
                .bind(&tax)
                .bind(changed)
                .execute(&mut *conn)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, \
                        unit, unit_price, product_id, tax_code, edited) \
                     VALUES ($1, $2, $3, $4, 'manual', $5, $6, $7, $8, $9, $10, true)",
                )
                .bind(huxley_core::new_id())
                .bind(id)
                .bind(current.org_id)
                .bind(i32::try_from(i).unwrap_or(0))
                .bind(&description)
                .bind(l.quantity)
                .bind(&unit)
                .bind(l.unit_price)
                .bind(l.product_id)
                .bind(&tax)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    sqlx::query(
        "UPDATE billing_invoices SET notes = $2, due_on = coalesce($3, due_on), version = version + 1, \
            revision = revision + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(text("notes", input.notes.as_deref(), 4000)?)
    .bind(input.due_on)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.updated")
            .org(current.org_id)
            .target("invoice", id)
            .details(json!({ "number": current.number, "lines": input.lines.len() })),
    )
    .await?;
    crate::review::refresh(&mut *conn, id).await?;
    get(conn, id).await
}

/// Holds a draft (it's left out of approval) or releases it.
pub async fn hold(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    on: bool,
    reason: Option<&str>,
) -> BillingResult<Invoice> {
    let i = lock(&mut *conn, id, None, &[Status::Draft], "held").await?;
    sqlx::query("UPDATE billing_invoices SET on_hold = $2, hold_reason = CASE WHEN $2 THEN $3 END, version = version + 1 WHERE id = $1")
        .bind(id)
        .bind(on)
        .bind(text("reason", reason, 500)?)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if on {
            "invoice.held"
        } else {
            "invoice.released"
        })
        .org(i.org_id)
        .target("invoice", id),
    )
    .await?;
    get(conn, id).await
}

async fn emit(conn: &mut PgConnection, event: &'static str, i: &Invoice) -> BillingResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(i.org_id),
            subject_type: "invoice",
            subject_id: i.id,
            payload: json!({
                "invoiceId": i.id, "reference": format!("INV-{}", i.number), "kind": i.kind, "orgId": i.org_id,
                "client": i.org_name, "status": i.status, "currency": i.currency,
                "total": i.total.normalize().to_string(), "link": format!("/billing/invoices/{}", i.id),
            }),
        },
    )
    .await?;
    Ok(())
}

/// Approves a draft that isn't held: it can then be sent or synced. One
/// that needs review takes an approved review of this revision, or
/// `override_review` (the caller checked the permission; it's recorded).
pub async fn approve(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
    override_review: bool,
) -> BillingResult<Invoice> {
    let i = lock(&mut *conn, id, Some(version), &[Status::Draft], "approved").await?;
    if i.on_hold {
        return Err(BillingError::Conflict(
            "it's on hold; release it first".into(),
        ));
    }
    if i.lines.is_empty() {
        return Err(BillingError::invalid("lines", "nothing to bill"));
    }
    crate::review::refresh(&mut *conn, id).await?;
    if !crate::review::may_approve(&mut *conn, id).await? {
        if !override_review {
            return Err(BillingError::Conflict(
                "it needs review: submit it for review first".into(),
            ));
        }
        crate::review::record_override(&mut *conn, ctx, &i).await?;
    }
    finish_approval(conn, ctx, &i).await
}

/// Approves a draft (its checks done): sets the issue date (today) and due
/// date (by the payment terms) if unset, and raises `invoice.approved`.
pub(crate) async fn finish_approval(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    i: &Invoice,
) -> BillingResult<Invoice> {
    let id = i.id;
    let terms = crate::settings::for_client(&mut *conn, i.org_id).await?;
    sqlx::query(
        "UPDATE billing_invoices SET status = 'approved', approved_by = $2, approved_at = now(), \
            issued_on = coalesce(issued_on, current_date), \
            due_on = coalesce(due_on, current_date + $3::integer), version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(ctx.actor.id())
    .bind(terms.payment_days)
    .execute(&mut *conn)
    .await?;
    let after = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.approved")
            .org(i.org_id)
            .target("invoice", id)
            .details(json!({ "number": i.number, "total": after.total.normalize().to_string() })),
    )
    .await?;
    emit(&mut *conn, "invoice.approved", &after).await?;
    crate::sync::on_approved(&mut *conn, ctx, &after).await?;
    get(conn, id).await
}

/// Marks an approved invoice sent (by hand, or by the accounting sync).
pub async fn mark_sent(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> BillingResult<Invoice> {
    let i = lock(&mut *conn, id, None, &[Status::Approved], "sent").await?;
    sqlx::query("UPDATE billing_invoices SET status = 'sent', version = version + 1, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let after = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.sent")
            .org(i.org_id)
            .target("invoice", id),
    )
    .await?;
    emit(&mut *conn, "invoice.sent", &after).await?;
    Ok(after)
}

/// Voids a draft or an approved invoice (not one that's gone out): what it
/// billed is given back, to bill again.
pub async fn void(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
) -> BillingResult<Invoice> {
    let i = lock(
        &mut *conn,
        id,
        Some(version),
        &[Status::Draft, Status::Approved],
        "voided",
    )
    .await?;
    sqlx::query(
        "DELETE FROM billing_billed WHERE line_id IN (SELECT id FROM billing_invoice_lines WHERE invoice_id = $1)",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE billing_invoices SET status = 'void', voided_at = now(), version = version + 1, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let after = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.voided")
            .org(i.org_id)
            .target("invoice", id)
            .details(json!({ "number": i.number })),
    )
    .await?;
    emit(&mut *conn, "invoice.voided", &after).await?;
    Ok(after)
}

/// A credit memo draft against an invoice that's gone out: the chosen
/// lines (all, if none are named), negated.
pub async fn credit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    lines: &[Uuid],
    reason: Option<&str>,
) -> BillingResult<Invoice> {
    let i = get(&mut *conn, id).await?;
    if i.kind != "invoice" || i.status != Status::Sent {
        return Err(BillingError::Conflict(
            "credit an invoice that's been sent; void a draft instead".into(),
        ));
    }
    let chosen: Vec<&Line> = i
        .lines
        .iter()
        .filter(|l| lines.is_empty() || lines.contains(&l.id))
        .collect();
    if chosen.is_empty() {
        return Err(BillingError::invalid(
            "lineIds",
            "none of those are its lines",
        ));
    }
    let memo = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO billing_invoices (id, org_id, kind, credit_of, currency, period_start, period_end, notes, created_by) \
         VALUES ($1, $2, 'credit_memo', $3, $4, $5, $6, $7, $8)",
    )
    .bind(memo)
    .bind(i.org_id)
    .bind(id)
    .bind(&i.currency)
    .bind(i.period_start)
    .bind(i.period_end)
    .bind(text("reason", reason, 4000)?.map(|r| format!("Credit for INV-{}: {r}", i.number)).or_else(|| Some(format!("Credit for INV-{}", i.number))))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    for (n, l) in chosen.iter().enumerate() {
        sqlx::query(
            "INSERT INTO billing_invoice_lines (id, invoice_id, org_id, position, kind, description, quantity, unit, \
                unit_price, product_id, tax_code, edited) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, false)",
        )
        .bind(huxley_core::new_id())
        .bind(memo)
        .bind(i.org_id)
        .bind(i32::try_from(n).unwrap_or(0))
        .bind(l.kind)
        .bind(&l.description)
        .bind(-l.quantity)
        .bind(&l.unit)
        .bind(l.unit_price)
        .bind(l.product_id)
        .bind(&l.tax_code)
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("invoice.credited")
            .org(i.org_id)
            .target("invoice", id)
            .details(json!({ "creditMemo": memo, "lines": chosen.len() })),
    )
    .await?;
    crate::review::refresh(&mut *conn, memo).await?;
    get(conn, memo).await
}
