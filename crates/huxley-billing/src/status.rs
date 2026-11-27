//! Payment status back-sync (ADR-078): the sync pack reads what's paid and
//! due in the accounting system onto each synced invoice; Huxley keeps each
//! client's balance and overdue amount from them.
//!
//! - The pack asks which synced invoices are still open ([`unpaid`], the
//!   `core.invoice.unpaid` step), reads them from the accounting system,
//!   and reports ([`record`], the `core.invoice.status` step) by the
//!   accounting system's IDs: amount due and paid, its status, and the
//!   client's view-and-pay link.
//! - An invoice whose amount due reaches zero raises `invoice.paid`.
//! - Each client's balance, overdue amount and oldest overdue due date are
//!   worked out again; past the provider's thresholds (days overdue, or an
//!   overdue total) it goes on credit hold (`company.credit_hold`), and
//!   comes off it (`company.credit_hold_released`) once it's back under.

use std::collections::BTreeSet;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::BillingResult;

/// A synced invoice that's still open.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Unpaid {
    pub invoice_id: Uuid,
    pub remote_id: String,
    pub reference: String,
    /// It's never been checked and has no view-and-pay link: fetch one
    /// (tried once, as not every accounting system has online payment on).
    pub needs_link: bool,
}

/// Synced invoices for `target` that aren't known to be paid, least recently
/// checked first: what the pack's status workflow reads.
pub async fn unpaid(
    conn: &mut PgConnection,
    target: &str,
    limit: i64,
) -> BillingResult<Vec<Unpaid>> {
    Ok(sqlx::query_as(
        "SELECT s.invoice_id, s.remote_id, 'INV-' || i.number AS reference, \
                (s.pay_url IS NULL AND s.status_checked_at IS NULL) AS needs_link \
           FROM billing_invoice_syncs s JOIN billing_invoices i ON i.id = s.invoice_id \
          WHERE s.target = $1 AND s.status = 'synced' AND s.remote_id IS NOT NULL \
            AND i.kind = 'invoice' AND i.status = 'sent' \
            AND (s.amount_due IS NULL OR s.amount_due <> 0) \
          ORDER BY s.status_checked_at NULLS FIRST, s.synced_at LIMIT $2",
    )
    .bind(target)
    .bind(limit.clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

/// What the accounting system says of one invoice; anything left out
/// stays as it was.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatusInput {
    pub remote_id: String,
    #[serde(default, deserialize_with = "crate::number_opt")]
    pub amount_due: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::number_opt")]
    pub amount_paid: Option<Decimal>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub pay_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    pub updated: usize,
    /// IDs the accounting system gave that match no synced invoice.
    pub unknown: Vec<String>,
    /// Invoices that are now paid.
    pub paid: Vec<Uuid>,
}

/// Records what the accounting system says of invoices synced to `target`,
/// then works out their clients' positions again.
pub async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    target: &str,
    statuses: &[StatusInput],
) -> BillingResult<Recorded> {
    let mut out = Recorded::default();
    let mut clients = BTreeSet::new();
    for s in statuses.iter().take(1000) {
        let found: Option<(Uuid, Uuid, Option<Decimal>, i64)> = sqlx::query_as(
            "SELECT s.invoice_id, s.org_id, s.amount_due, i.number FROM billing_invoice_syncs s \
               JOIN billing_invoices i ON i.id = s.invoice_id \
              WHERE s.target = $1 AND s.remote_id = $2 AND s.status = 'synced' \
              ORDER BY s.synced_at DESC LIMIT 1",
        )
        .bind(target)
        .bind(s.remote_id.trim())
        .fetch_optional(&mut *conn)
        .await?;
        let Some((invoice, org, before, number)) = found else {
            out.unknown.push(s.remote_id.clone());
            continue;
        };
        let pay_url = s
            .pay_url
            .as_deref()
            .map(str::trim)
            .filter(|u| u.starts_with("https://") && u.len() <= 2000);
        let status = s
            .status
            .as_deref()
            .map(|t| t.trim().chars().take(50).collect::<String>());
        let paid_now: bool = sqlx::query_scalar(
            "UPDATE billing_invoice_syncs SET amount_due = coalesce($2, amount_due), \
                amount_paid = coalesce($3, amount_paid), remote_status = coalesce($4, remote_status), \
                pay_url = coalesce($5, pay_url), status_checked_at = now(), \
                paid_on = CASE WHEN coalesce($2, amount_due) = 0 THEN coalesce(paid_on, current_date) END \
              WHERE invoice_id = $1 RETURNING coalesce(amount_due, 1) = 0",
        )
        .bind(invoice)
        .bind(s.amount_due)
        .bind(s.amount_paid)
        .bind(&status)
        .bind(pay_url)
        .fetch_one(&mut *conn)
        .await?;
        out.updated += 1;
        clients.insert(org);
        if paid_now && before != Some(Decimal::ZERO) {
            out.paid.push(invoice);
            outbox::emit(
                &mut *conn,
                DomainEvent {
                    event_type: "invoice.paid",
                    org_id: Some(org),
                    subject_type: "invoice",
                    subject_id: invoice,
                    payload: json!({
                        "invoiceId": invoice, "reference": format!("INV-{number}"), "orgId": org,
                        "target": target, "link": format!("/billing/invoices/{invoice}"),
                    }),
                },
            )
            .await?;
        }
    }
    for org in clients {
        recompute(&mut *conn, ctx, org).await?;
    }
    Ok(out)
}

/// A client's position from its synced invoices.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub org_id: Uuid,
    pub org_name: String,
    pub currency: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub balance: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub overdue: Decimal,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub oldest_due: Option<Date>,
    pub credit_hold: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub hold_since: Option<OffsetDateTime>,
}

const CLIENT: &str = "SELECT c.org_id, o.name AS org_name, c.currency, c.balance, c.overdue, c.oldest_due, \
        c.credit_hold, c.hold_since FROM billing_client_status c JOIN organizations o ON o.id = c.org_id";

pub async fn client_status(
    conn: &mut PgConnection,
    org: Uuid,
) -> BillingResult<Option<ClientStatus>> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{CLIENT} WHERE c.org_id = $1")))
            .bind(org)
            .fetch_optional(conn)
            .await?,
    )
}

/// Clients with something owing, most overdue first.
pub async fn receivables(conn: &mut PgConnection) -> BillingResult<Vec<ClientStatus>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{CLIENT} WHERE c.balance <> 0 OR c.credit_hold ORDER BY c.overdue DESC, c.balance DESC LIMIT 500"
    )))
    .fetch_all(conn)
    .await?)
}

/// Works out a client's balance, overdue amount and credit hold again.
pub async fn recompute(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org: Uuid,
) -> BillingResult<Option<ClientStatus>> {
    let (currency, balance, overdue, oldest): (Option<String>, Decimal, Decimal, Option<Date>) =
        sqlx::query_as(
            "SELECT max(i.currency), coalesce(sum(s.amount_due), 0), \
                    coalesce(sum(s.amount_due) FILTER (WHERE i.due_on < current_date AND s.amount_due > 0), 0), \
                    min(i.due_on) FILTER (WHERE i.due_on < current_date AND s.amount_due > 0) \
               FROM billing_invoice_syncs s JOIN billing_invoices i ON i.id = s.invoice_id \
              WHERE s.org_id = $1 AND s.status = 'synced' AND i.kind = 'invoice' AND i.status = 'sent'",
        )
        .bind(org)
        .fetch_one(&mut *conn)
        .await?;
    let thresholds: Option<(Option<i32>, Option<Decimal>)> = sqlx::query_as(
        "SELECT s.credit_hold_days, s.credit_hold_amount FROM billing_sync_settings s \
           JOIN organizations so ON so.id = s.org_id JOIN organizations c ON c.id = $1 \
          WHERE so.path @> c.path ORDER BY nlevel(so.path) DESC LIMIT 1",
    )
    .bind(org)
    .fetch_optional(&mut *conn)
    .await?;
    let (days, amount) = thresholds.unwrap_or_default();
    let today = OffsetDateTime::now_utc().date();
    let late = days
        .zip(oldest)
        .is_some_and(|(d, o)| (today - o).whole_days() >= i64::from(d));
    let over = amount.is_some_and(|a| overdue > a);
    let hold = late || over;
    let was: Option<bool> =
        sqlx::query_scalar("SELECT credit_hold FROM billing_client_status WHERE org_id = $1")
            .bind(org)
            .fetch_optional(&mut *conn)
            .await?;
    sqlx::query(
        "INSERT INTO billing_client_status (org_id, currency, balance, overdue, oldest_due, credit_hold, hold_since) \
         VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $6 THEN now() END) \
         ON CONFLICT (org_id) DO UPDATE SET currency = $2, balance = $3, overdue = $4, oldest_due = $5, credit_hold = $6, \
            hold_since = CASE WHEN $6 THEN coalesce(billing_client_status.hold_since, now()) END, updated_at = now()",
    )
    .bind(org)
    .bind(currency.unwrap_or_else(|| "USD".into()))
    .bind(balance)
    .bind(overdue)
    .bind(oldest)
    .bind(hold)
    .execute(&mut *conn)
    .await?;
    if was.unwrap_or(false) != hold {
        let event = if hold {
            "company.credit_hold"
        } else {
            "company.credit_hold_released"
        };
        let why = if late {
            format!("an invoice is {} days overdue", days.unwrap_or_default())
        } else if over {
            format!("{} is overdue", overdue.normalize())
        } else {
            "back under the credit hold limits".to_owned()
        };
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: event,
                org_id: Some(org),
                subject_type: "organization",
                subject_id: org,
                payload: json!({
                    "orgId": org, "balance": balance.normalize().to_string(),
                    "overdue": overdue.normalize().to_string(),
                    "oldestDue": oldest.map(|d| d.to_string()), "reason": why,
                }),
            },
        )
        .await?;
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new(event)
                .org(org)
                .target("organization", org)
                .details(json!({ "overdue": overdue.normalize().to_string(), "reason": why })),
        )
        .await?;
    }
    client_status(conn, org).await
}

// ---- the client portal ----

/// An invoice in the client portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalInvoice {
    pub id: Uuid,
    pub number: i64,
    pub kind: String,
    pub currency: String,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub issued_on: Option<Date>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub due_on: Option<Date>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub total: Decimal,
    /// What's still owed, when the accounting system has said.
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub amount_due: Option<Decimal>,
    /// `paid`, `overdue`, `due` or `credit`.
    pub state: String,
    /// Where to view and pay it.
    pub pay_url: Option<String>,
}

/// The company's invoices and credit memos that have gone out, newest first.
pub async fn portal_list(conn: &mut PgConnection, org: Uuid) -> BillingResult<Vec<PortalInvoice>> {
    Ok(sqlx::query_as(
        "SELECT i.id, i.number, i.kind, i.currency, i.issued_on, i.due_on, \
                coalesce((SELECT sum(round(l.quantity * l.unit_price, 2)) FROM billing_invoice_lines l \
                           WHERE l.invoice_id = i.id), 0) AS total, \
                s.amount_due, \
                CASE WHEN i.kind = 'credit_memo' THEN 'credit' \
                     WHEN s.amount_due = 0 THEN 'paid' \
                     WHEN i.due_on < current_date THEN 'overdue' ELSE 'due' END AS state, \
                s.pay_url \
           FROM billing_invoices i LEFT JOIN billing_invoice_syncs s ON s.invoice_id = i.id \
          WHERE i.org_id = $1 AND i.status = 'sent' ORDER BY i.issued_on DESC NULLS LAST, i.number DESC LIMIT 200",
    )
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// One of the company's invoices that's gone out, if it is one.
pub async fn portal_owns(conn: &mut PgConnection, org: Uuid, id: Uuid) -> BillingResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM billing_invoices WHERE id = $1 AND org_id = $2 AND status = 'sent')",
    )
    .bind(id)
    .bind(org)
    .fetch_one(conn)
    .await?)
}
