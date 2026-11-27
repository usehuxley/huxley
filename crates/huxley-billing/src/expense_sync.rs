//! Paying expenses back (ADR-083): an approved expense report is sent to
//! the accounting system as a bill to the person (mapped to a supplier),
//! one line per reimbursable expense on its category's expense account,
//! through the same pack workflows as invoices (ADR-077); or it's marked
//! paid back by hand. Reports can also be exported as CSV.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::sync::{self, MappingKind, Outcome};
use crate::{BillingError, BillingResult};

/// A report's accounting status.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ReportSync {
    pub report_id: Uuid,
    pub status: String,
    pub sync_target: Option<String>,
    /// `pending`, `synced` or `failed`; none if it was never sent.
    pub sync_status: Option<String>,
    pub sync_error: Option<String>,
    pub remote_id: Option<String>,
    pub remote_number: Option<String>,
    pub remote_url: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reimbursed_at: Option<OffsetDateTime>,
}

pub async fn state(conn: &mut PgConnection, report: Uuid) -> BillingResult<ReportSync> {
    sqlx::query_as(
        "SELECT id AS report_id, status, sync_target, sync_status, sync_error, remote_id, remote_number, remote_url, \
                reimbursed_at FROM exp_reports WHERE id = $1",
    )
    .bind(report)
    .fetch_optional(conn)
    .await?
    .ok_or(BillingError::NotFound)
}

/// A reimbursable expense, for the bill.
#[derive(sqlx::FromRow)]
struct Line {
    category_id: Uuid,
    category: String,
    description: String,
    spent_on: Date,
    amount: Decimal,
    tax: Decimal,
    currency: String,
}

/// The report as the sync workflow gets it, or why it can't be.
async fn prepare(
    conn: &mut PgConnection,
    report: Uuid,
    provider: Uuid,
    target: &str,
) -> BillingResult<Result<Json, String>> {
    let (number, title, user, person, decided): (
        i64,
        String,
        Uuid,
        String,
        Option<OffsetDateTime>,
    ) = sqlx::query_as(
        "SELECT r.number, r.title, r.user_id, u.display_name, r.decided_at FROM exp_reports r \
               JOIN users u ON u.id = r.user_id WHERE r.id = $1",
    )
    .bind(report)
    .fetch_one(&mut *conn)
    .await?;
    let lines: Vec<Line> = sqlx::query_as(
        "SELECT e.category_id, c.name AS category, e.description, e.spent_on, e.amount, e.tax, e.currency \
           FROM exp_entries e JOIN exp_categories c ON c.id = e.category_id \
          WHERE e.report_id = $1 AND e.reimbursable ORDER BY e.spent_on, e.created_at",
    )
    .bind(report)
    .fetch_all(&mut *conn)
    .await?;
    if lines.is_empty() {
        return Ok(Err("nothing in it is to be paid back".into()));
    }
    let currency = lines[0].currency.clone();
    if lines.iter().any(|l| l.currency != currency) {
        return Ok(Err(
            "its expenses are in more than one currency: pay it back by hand".into(),
        ));
    }
    let Some(supplier) = sync::mapped(
        &mut *conn,
        provider,
        target,
        MappingKind::Supplier,
        &[user.to_string()],
    )
    .await?
    else {
        return Ok(Err(format!(
            "{person} isn't matched to a supplier in {target}: map them under Billing → Accounting"
        )));
    };
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for l in &lines {
        let keys = [l.category_id.to_string(), "default".into()];
        let Some(account) =
            sync::mapped(&mut *conn, provider, target, MappingKind::Account, &keys).await?
        else {
            missing.push(l.category.clone());
            continue;
        };
        out.push(json!({
            "description": format!("{}: {} ({})", l.category, l.description, l.spent_on),
            "category": l.category,
            "spentOn": l.spent_on.to_string(),
            "amount": sync::money(l.amount),
            "tax": sync::money(l.tax),
            "account": sync::reference(account),
        }));
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Ok(Err(format!(
            "no expense account in {target} for {}: map the categories (or a default) under Billing → Accounting",
            missing.join(", ")
        )));
    }
    let total: Decimal = lines.iter().map(|l| l.amount).sum();
    let day = decided.unwrap_or_else(OffsetDateTime::now_utc).date();
    Ok(Ok(json!({
        "reportId": report,
        "target": target,
        "reference": format!("EXP-{number}"),
        "title": title,
        "person": person,
        "supplier": sync::reference(supplier),
        "date": day.to_string(),
        "currency": currency,
        "lines": out,
        "total": sync::money(total),
        "link": format!("/expenses/reports/{report}"),
    })))
}

/// Sends an approved report to the accounting system: raises
/// `expense_report.sync_requested`, or fails it with the reason.
pub async fn request(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    report: Uuid,
) -> BillingResult<ReportSync> {
    let current = state(&mut *conn, report).await?;
    if current.status != "approved" {
        return Err(BillingError::Conflict(
            "only an approved report can be sent to accounting".into(),
        ));
    }
    if current.sync_status.as_deref() == Some("synced") {
        return Err(BillingError::Conflict(
            "it's already in the accounting system".into(),
        ));
    }
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM exp_reports WHERE id = $1")
        .bind(report)
        .fetch_one(&mut *conn)
        .await?;
    let Some((provider, settings)) = sync::for_client(&mut *conn, org).await? else {
        return Err(BillingError::Conflict(
            "no accounting system is set up: choose one in Settings → Billing".into(),
        ));
    };
    let prepared = prepare(&mut *conn, report, provider, &settings.target).await?;
    let (status, error) = match &prepared {
        Ok(_) => ("pending", None),
        Err(why) => ("failed", Some(why.clone())),
    };
    sqlx::query(
        "UPDATE exp_reports SET sync_target = $2, sync_status = $3, sync_error = $4 WHERE id = $1",
    )
    .bind(report)
    .bind(&settings.target)
    .bind(status)
    .bind(&error)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_report.sync_requested")
            .org(org)
            .target("expense_report", report)
            .details(json!({ "target": settings.target, "error": error })),
    )
    .await?;
    if let Ok(payload) = prepared {
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "expense_report.sync_requested",
                org_id: Some(org),
                subject_type: "expense_report",
                subject_id: report,
                payload,
            },
        )
        .await?;
    }
    state(conn, report).await
}

/// Records how sending a report went (from the pack's workflow).
pub async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    report: Uuid,
    outcome: &Outcome,
) -> BillingResult<ReportSync> {
    let current = state(&mut *conn, report).await?;
    if current.sync_status.as_deref() == Some("synced") {
        return Err(BillingError::Conflict(
            "it's already in the accounting system".into(),
        ));
    }
    let clip = |s: Option<&str>, n: usize| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(n).collect::<String>())
    };
    if outcome.ok {
        let remote = clip(outcome.remote_id.as_deref(), 200)
            .ok_or_else(|| BillingError::invalid("remote_id", "required when it synced"))?;
        sqlx::query(
            "UPDATE exp_reports SET sync_status = 'synced', sync_error = NULL, remote_id = $2, remote_number = $3, \
                remote_url = $4, synced_at = now() WHERE id = $1",
        )
        .bind(report)
        .bind(&remote)
        .bind(clip(outcome.remote_number.as_deref(), 100))
        .bind(clip(outcome.remote_url.as_deref(), 2000).filter(|u| u.starts_with("https://")))
        .execute(&mut *conn)
        .await?;
    } else {
        let why = clip(outcome.error.as_deref(), 4000)
            .unwrap_or_else(|| "the sync failed without saying why".into());
        sqlx::query("UPDATE exp_reports SET sync_status = 'failed', sync_error = $2 WHERE id = $1")
            .bind(report)
            .bind(&why)
            .execute(&mut *conn)
            .await?;
    }
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM exp_reports WHERE id = $1")
        .bind(report)
        .fetch_one(&mut *conn)
        .await?;
    let event = if outcome.ok {
        "expense_report.synced"
    } else {
        "expense_report.sync_failed"
    };
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(org)
            .target("expense_report", report)
            .details(json!({ "remoteId": outcome.remote_id, "error": outcome.error })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: event,
            org_id: Some(org),
            subject_type: "expense_report",
            subject_id: report,
            payload: json!({ "reportId": report, "remoteId": outcome.remote_id, "error": outcome.error,
                             "link": format!("/expenses/reports/{report}") }),
        },
    )
    .await?;
    state(conn, report).await
}

/// Marks an approved report paid back: it and its expenses are reimbursed.
pub async fn mark_reimbursed(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    report: Uuid,
) -> BillingResult<ReportSync> {
    let current = state(&mut *conn, report).await?;
    if current.status != "approved" {
        return Err(BillingError::Conflict(
            "only an approved report can be paid back".into(),
        ));
    }
    let org: Uuid = sqlx::query_scalar(
        "UPDATE exp_reports SET status = 'reimbursed', reimbursed_at = now(), reimbursed_by = $2, version = version + 1 \
          WHERE id = $1 RETURNING org_id",
    )
    .bind(report)
    .bind(ctx.actor.id())
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("UPDATE exp_entries SET status = 'reimbursed', version = version + 1, updated_at = now() WHERE report_id = $1")
        .bind(report)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_report.reimbursed")
            .org(org)
            .target("expense_report", report),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "expense_report.reimbursed",
            org_id: Some(org),
            subject_type: "expense_report",
            subject_id: report,
            payload: json!({ "reportId": report, "link": format!("/expenses/reports/{report}") }),
        },
    )
    .await?;
    state(conn, report).await
}

/// One reimbursable expense, for export.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExportRow {
    pub report: String,
    pub person: String,
    pub spent_on: Date,
    pub category: String,
    pub description: String,
    pub vendor: Option<String>,
    pub amount: Decimal,
    pub tax: Decimal,
    pub currency: String,
    pub client: String,
    pub status: String,
}

/// Reimbursable expenses in reports with `status` (approved, by default),
/// as CSV.
pub async fn export_csv(conn: &mut PgConnection, status: Option<&str>) -> BillingResult<String> {
    let rows: Vec<ExportRow> = sqlx::query_as(
        "SELECT 'EXP-' || r.number AS report, u.display_name AS person, e.spent_on, c.name AS category, e.description, \
                e.vendor, e.amount, e.tax, e.currency, o.name AS client, r.status \
           FROM exp_reports r JOIN exp_entries e ON e.report_id = r.id JOIN users u ON u.id = r.user_id \
           JOIN exp_categories c ON c.id = e.category_id JOIN organizations o ON o.id = e.org_id \
          WHERE r.status = $1 AND e.reimbursable ORDER BY r.number, e.spent_on",
    )
    .bind(status.unwrap_or("approved"))
    .fetch_all(conn)
    .await?;
    let cell = |s: &str| {
        let s = if s.starts_with(['=', '+', '-', '@']) {
            format!("'{s}")
        } else {
            s.to_owned()
        };
        format!("\"{}\"", s.replace('"', "\"\""))
    };
    let mut out = String::from(
        "report,person,date,category,description,vendor,amount,tax,currency,client,status\n",
    );
    for r in rows {
        let fields = [
            cell(&r.report),
            cell(&r.person),
            r.spent_on.to_string(),
            cell(&r.category),
            cell(&r.description),
            cell(r.vendor.as_deref().unwrap_or("")),
            r.amount.normalize().to_string(),
            r.tax.normalize().to_string(),
            r.currency,
            cell(&r.client),
            r.status,
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    Ok(out)
}
