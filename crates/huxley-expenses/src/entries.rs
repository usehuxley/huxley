//! Expense entries: what someone spent (or drove), where it goes, and its
//! receipt.

use bytes::Bytes;
use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_storage::Storage;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::categories::{self, Kind};
use crate::{ExpenseError, ExpenseResult, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ExpenseStatus)]
pub enum Status {
    Draft,
    Submitted,
    Approved,
    Rejected,
    Reimbursed,
}

#[allow(clippy::struct_excessive_bools)] // independent settings, one per column
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Expense {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub user_id: Uuid,
    pub user_name: String,
    pub category_id: Uuid,
    pub category_name: String,
    pub kind: Kind,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub spent_on: Date,
    pub description: String,
    pub vendor: Option<String>,
    /// What was paid, tax included.
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub tax: Decimal,
    pub currency: String,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub distance: Option<Decimal>,
    pub unit: Option<String>,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub rate: Option<Decimal>,
    pub billable: bool,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub markup_pct: Decimal,
    /// What the client is charged: the amount marked up (when billable).
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub bill_amount: Decimal,
    pub reimbursable: bool,
    pub ticket_id: Option<Uuid>,
    pub ticket_ref: Option<String>,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub phase_id: Option<Uuid>,
    pub phase_name: Option<String>,
    pub has_receipt: bool,
    pub receipt_name: Option<String>,
    /// The receipt's malware scan (ADR-094), when there's a receipt:
    /// `unscanned`, `pending`, `clean`, `infected` (quarantined) or `failed`.
    pub receipt_scan: Option<String>,
    /// What AI read from the receipt, for the person to confirm (ADR-080).
    #[schema(value_type = Option<Reading>)]
    #[sqlx(json(nullable))]
    pub reading: Option<Reading>,
    pub reading_error: Option<String>,
    pub status: Status,
    /// The report it was submitted in.
    pub report_id: Option<Uuid>,
    /// Policy flags: `over_limit`, `no_receipt`.
    #[sqlx(skip)]
    pub flags: Vec<String>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(skip)]
    #[schema(ignore)]
    limit_amount: Option<Decimal>,
    #[serde(skip)]
    #[schema(ignore)]
    receipt_required: bool,
}

impl Expense {
    fn flagged(mut self) -> Self {
        if self.limit_amount.is_some_and(|l| self.amount > l) {
            self.flags.push("over_limit".into());
        }
        if self.receipt_required && self.kind == Kind::Expense && !self.has_receipt {
            self.flags.push("no_receipt".into());
        }
        self
    }
}

const SELECT: &str = "SELECT e.id, e.org_id, o.name AS org_name, e.user_id, u.display_name AS user_name, e.category_id, \
        c.name AS category_name, c.kind, e.spent_on, e.description, e.vendor, e.amount, e.tax, e.currency, \
        e.distance, e.unit, e.rate, e.billable, e.markup_pct, \
        CASE WHEN e.billable THEN round(e.amount * (1 + e.markup_pct / 100), 2) ELSE 0 END AS bill_amount, \
        e.reimbursable, e.ticket_id, (t.number_prefix || '-' || t.number) AS ticket_ref, \
        e.project_id, p.name AS project_name, e.phase_id, ph.name AS phase_name, \
        (e.receipt_key IS NOT NULL) AS has_receipt, e.receipt_name, \
        CASE WHEN e.receipt_key IS NOT NULL THEN coalesce(rs.status, 'unscanned') END AS receipt_scan, e.reading, e.reading_error, e.status, e.report_id, e.version, e.created_at, \
        c.limit_amount, c.receipt_required \
   FROM exp_entries e JOIN organizations o ON o.id = e.org_id JOIN users u ON u.id = e.user_id \
   JOIN exp_categories c ON c.id = e.category_id LEFT JOIN sd_tickets t ON t.id = e.ticket_id \
   LEFT JOIN prj_projects p ON p.id = e.project_id LEFT JOIN prj_phases ph ON ph.id = e.phase_id \
   LEFT JOIN file_scans rs ON rs.storage_key = e.receipt_key";

/// What was read from a receipt. Anything it couldn't read is null.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// The total paid, tax included, as a decimal string.
    pub amount: Option<String>,
    pub tax: Option<String>,
    pub currency: Option<String>,
    /// `YYYY-MM-DD`.
    pub spent_on: Option<String>,
    pub vendor: Option<String>,
    /// What was bought, briefly.
    pub description: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    /// Whose (the API sets it to you unless you may see everyone's).
    pub user_id: Option<Uuid>,
    pub ticket_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub status: Option<Status>,
    #[serde(default, with = "crate::date::option")]
    #[param(value_type = Option<String>, format = Date)]
    pub from: Option<Date>,
    #[serde(default, with = "crate::date::option")]
    #[param(value_type = Option<String>, format = Date)]
    pub to: Option<Date>,
}

pub async fn list(conn: &mut PgConnection, f: &Filter) -> ExpenseResult<Vec<Expense>> {
    let rows: Vec<Expense> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR e.user_id = $1) AND ($2::uuid IS NULL OR e.ticket_id = $2) \
            AND ($3::uuid IS NULL OR e.project_id = $3) AND ($4::uuid IS NULL OR e.org_id = $4) \
            AND ($5::text IS NULL OR e.status = $5) AND ($6::date IS NULL OR e.spent_on >= $6) \
            AND ($7::date IS NULL OR e.spent_on <= $7) \
          ORDER BY e.spent_on DESC, e.created_at DESC LIMIT 500"
    )))
    .bind(f.user_id)
    .bind(f.ticket_id)
    .bind(f.project_id)
    .bind(f.org_id)
    .bind(f.status)
    .bind(f.from)
    .bind(f.to)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Expense::flagged).collect())
}

/// A report's entries, by day.
pub async fn in_report(conn: &mut PgConnection, report: Uuid) -> ExpenseResult<Vec<Expense>> {
    let rows: Vec<Expense> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE e.report_id = $1 ORDER BY e.spent_on, e.created_at"
    )))
    .bind(report)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Expense::flagged).collect())
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<Expense> {
    let e: Expense = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE e.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(ExpenseError::NotFound)?;
    Ok(e.flagged())
}

/// Who an entry belongs to and its organization: `(user, org)`.
pub async fn owner_of(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<Option<(Uuid, Uuid)>> {
    Ok(
        sqlx::query_as("SELECT user_id, org_id FROM exp_entries WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpenseInput {
    pub category_id: Uuid,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub spent_on: Date,
    pub description: String,
    #[serde(default)]
    pub vendor: Option<String>,
    /// What was paid, tax included (not for mileage).
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub amount: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub tax: Option<Decimal>,
    #[serde(default)]
    pub currency: Option<String>,
    /// Mileage: the distance.
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub distance: Option<Decimal>,
    /// Mileage: `mi` (the default) or `km`.
    #[serde(default)]
    pub unit: Option<String>,
    /// Default: the category's.
    #[serde(default)]
    pub billable: Option<bool>,
    #[serde(default)]
    pub reimbursable: Option<bool>,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub markup_pct: Option<Decimal>,
    /// Against a ticket (its client)…
    #[serde(default)]
    pub ticket_id: Option<Uuid>,
    /// …or a project (and optionally one of its phases)…
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub phase_id: Option<Uuid>,
    /// …or a client, or the provider (internal).
    #[serde(default)]
    pub org_id: Option<Uuid>,
    /// Updates: the version read.
    #[serde(default)]
    pub version: Option<i32>,
}

/// What an entry is against, worked out: `(org, ticket, project, phase)`.
pub async fn placement(
    conn: &mut PgConnection,
    input: &ExpenseInput,
) -> ExpenseResult<(Uuid, Option<Uuid>, Option<Uuid>, Option<Uuid>)> {
    if let Some(ticket) = input.ticket_id {
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT t.org_id, s.category FROM sd_tickets t JOIN sd_statuses s ON s.id = t.status_id WHERE t.id = $1",
        )
        .bind(ticket)
        .fetch_optional(&mut *conn)
        .await?;
        let (org, category) =
            row.ok_or_else(|| ExpenseError::invalid("ticketId", "no such ticket"))?;
        if category == "closed" {
            return Err(ExpenseError::invalid("ticketId", "the ticket is closed"));
        }
        let project: Option<Uuid> =
            sqlx::query_scalar("SELECT project_id FROM prj_tasks WHERE ticket_id = $1")
                .bind(ticket)
                .fetch_optional(&mut *conn)
                .await?;
        return Ok((org, Some(ticket), project.or(input.project_id), None));
    }
    if let Some(project) = input.project_id {
        let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM prj_projects WHERE id = $1")
            .bind(project)
            .fetch_optional(&mut *conn)
            .await?;
        let org = org.ok_or_else(|| ExpenseError::invalid("projectId", "no such project"))?;
        if let Some(phase) = input.phase_id {
            let ok: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM prj_phases WHERE id = $1 AND project_id = $2)",
            )
            .bind(phase)
            .bind(project)
            .fetch_one(&mut *conn)
            .await?;
            if !ok {
                return Err(ExpenseError::invalid(
                    "phaseId",
                    "not a phase of this project",
                ));
            }
        }
        return Ok((org, None, Some(project), input.phase_id));
    }
    let org = input.org_id.ok_or_else(|| {
        ExpenseError::invalid(
            "orgId",
            "say what it's for: a ticket, a project, a client or your MSP",
        )
    })?;
    Ok((org, None, None, None))
}

/// The entry's money, worked out: `(amount, tax, currency, distance, unit, rate)`.
type Money = (
    Decimal,
    Decimal,
    String,
    Option<Decimal>,
    Option<String>,
    Option<Decimal>,
);

async fn money(conn: &mut PgConnection, kind: Kind, input: &ExpenseInput) -> ExpenseResult<Money> {
    let currency = crate::currency(input.currency.as_deref())?;
    let tax = input.tax.unwrap_or_default();
    if tax < Decimal::ZERO {
        return Err(ExpenseError::invalid("tax", "zero or more"));
    }
    match kind {
        Kind::Mileage => {
            let distance = input
                .distance
                .filter(|d| *d > Decimal::ZERO)
                .ok_or_else(|| ExpenseError::invalid("distance", "how far, more than zero"))?;
            let unit = crate::rates::unit(input.unit.as_deref())?;
            let rate = crate::rates::on(&mut *conn, &unit, &currency, input.spent_on)
                .await?
                .ok_or_else(|| {
                    ExpenseError::invalid(
                        "distance",
                        format!("no mileage rate per {unit} in {currency} on that day"),
                    )
                })?;
            Ok((
                (distance * rate).round_dp(2),
                Decimal::ZERO,
                currency,
                Some(distance),
                Some(unit),
                Some(rate),
            ))
        }
        Kind::Expense => {
            let amount = input
                .amount
                .filter(|a| *a >= Decimal::ZERO)
                .ok_or_else(|| ExpenseError::invalid("amount", "what was paid, zero or more"))?;
            if tax > amount {
                return Err(ExpenseError::invalid(
                    "tax",
                    "can't be more than the amount",
                ));
            }
            Ok((amount, tax, currency, None, None, None))
        }
    }
}

fn check_day(day: Date) -> ExpenseResult<()> {
    let today = OffsetDateTime::now_utc().date();
    if day > today || (today - day).whole_days() > 366 {
        return Err(ExpenseError::invalid(
            "spentOn",
            "within the past year, and not ahead",
        ));
    }
    Ok(())
}

/// Logs an expense for `user`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    input: &ExpenseInput,
) -> ExpenseResult<Expense> {
    let category = categories::get(&mut *conn, input.category_id).await?;
    if !category.active {
        return Err(ExpenseError::invalid(
            "categoryId",
            "that category is no longer used",
        ));
    }
    check_day(input.spent_on)?;
    let description = required("description", &input.description, 500)?;
    let vendor = text("vendor", input.vendor.as_deref(), 200)?;
    let (org, ticket, project, phase) = placement(&mut *conn, input).await?;
    let (amount, tax, currency, distance, unit, rate) =
        money(&mut *conn, category.kind, input).await?;
    let markup = input.markup_pct.unwrap_or(category.markup_pct);
    if markup < Decimal::ZERO {
        return Err(ExpenseError::invalid("markupPct", "zero or more"));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO exp_entries (id, org_id, user_id, category_id, spent_on, description, vendor, amount, tax, currency, \
            distance, unit, rate, billable, markup_pct, reimbursable, ticket_id, project_id, phase_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)",
    )
    .bind(id)
    .bind(org)
    .bind(user)
    .bind(category.id)
    .bind(input.spent_on)
    .bind(&description)
    .bind(&vendor)
    .bind(amount)
    .bind(tax)
    .bind(&currency)
    .bind(distance)
    .bind(&unit)
    .bind(rate)
    .bind(input.billable.unwrap_or(category.billable))
    .bind(markup)
    .bind(input.reimbursable.unwrap_or(category.reimbursable))
    .bind(ticket)
    .bind(project)
    .bind(phase)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.created")
            .org(org)
            .target("expense", id)
            .details(json!({ "category": category.name, "amount": amount.normalize().to_string(), "currency": currency })),
    )
    .await?;
    get(conn, id).await
}

/// The entry, locked, if it can still be changed (a draft, or sent back).
async fn editable(
    conn: &mut PgConnection,
    id: Uuid,
    version: Option<i32>,
) -> ExpenseResult<Expense> {
    let (status, current): (Status, i32) =
        sqlx::query_as("SELECT status, version FROM exp_entries WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(ExpenseError::NotFound)?;
    if version.is_some_and(|v| v != current) {
        return Err(ExpenseError::Conflict(
            "it was changed since you loaded it; reload and try again".into(),
        ));
    }
    if !matches!(status, Status::Draft | Status::Rejected) {
        return Err(ExpenseError::Conflict(
            "it's been submitted; only drafts (or ones sent back) can be changed".into(),
        ));
    }
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ExpenseInput,
) -> ExpenseResult<Expense> {
    let current = editable(&mut *conn, id, input.version).await?;
    let category = categories::get(&mut *conn, input.category_id).await?;
    check_day(input.spent_on)?;
    let description = required("description", &input.description, 500)?;
    let vendor = text("vendor", input.vendor.as_deref(), 200)?;
    let (org, ticket, project, phase) = placement(&mut *conn, input).await?;
    let (amount, tax, currency, distance, unit, rate) =
        money(&mut *conn, category.kind, input).await?;
    sqlx::query(
        "UPDATE exp_entries SET org_id = $2, category_id = $3, spent_on = $4, description = $5, vendor = $6, amount = $7, \
            tax = $8, currency = $9, distance = $10, unit = $11, rate = $12, billable = $13, markup_pct = $14, \
            reimbursable = $15, ticket_id = $16, project_id = $17, phase_id = $18, version = version + 1, \
            updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(org)
    .bind(category.id)
    .bind(input.spent_on)
    .bind(&description)
    .bind(&vendor)
    .bind(amount)
    .bind(tax)
    .bind(&currency)
    .bind(distance)
    .bind(&unit)
    .bind(rate)
    .bind(input.billable.unwrap_or(current.billable))
    .bind(input.markup_pct.unwrap_or(current.markup_pct))
    .bind(input.reimbursable.unwrap_or(current.reimbursable))
    .bind(ticket)
    .bind(project)
    .bind(phase)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.updated")
            .org(org)
            .target("expense", id)
            .details(json!({ "amount": amount.normalize().to_string() })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a draft, and its receipt.
pub async fn delete(
    conn: &mut PgConnection,
    storage: &Storage,
    ctx: &RequestContext,
    id: Uuid,
) -> ExpenseResult<()> {
    let e = editable(&mut *conn, id, None).await?;
    let key: Option<String> =
        sqlx::query_scalar("DELETE FROM exp_entries WHERE id = $1 RETURNING receipt_key")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.deleted")
            .org(e.org_id)
            .target("expense", id),
    )
    .await?;
    if let Some(key) = key {
        let _ = storage.delete(&key).await;
    }
    Ok(())
}

/// What a receipt can be.
fn receipt_type(content_type: &str) -> bool {
    // Not SVG: it can carry script.
    matches!(
        content_type,
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "image/heic"
            | "image/heif"
            | "application/pdf"
    )
}

/// Keeps a receipt (an image or a PDF) on an entry, replacing any before.
pub async fn put_receipt(
    conn: &mut PgConnection,
    storage: &Storage,
    ctx: &RequestContext,
    id: Uuid,
    filename: &str,
    content_type: &str,
    data: Bytes,
) -> ExpenseResult<Expense> {
    let e = editable(&mut *conn, id, None).await?;
    if !receipt_type(content_type) {
        return Err(ExpenseError::invalid("receipt", "a photo or a PDF"));
    }
    let max = storage.max_upload_bytes();
    if data.len() as u64 > max {
        return Err(ExpenseError::TooLarge(max / (1024 * 1024)));
    }
    let name: String = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("receipt")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    let name = if name.trim().is_empty() {
        "receipt".to_owned()
    } else {
        name
    };
    let key = format!("expenses/{id}/{}", huxley_core::new_id());
    let size = i64::try_from(data.len()).unwrap_or(i64::MAX);
    storage.put(&key, data).await?;
    let old: Option<String> = sqlx::query_scalar(
        "UPDATE exp_entries e SET receipt_key = $2, receipt_name = $3, receipt_type = $4, receipt_size = $5, \
            version = version + 1, updated_at = now() \
          FROM (SELECT receipt_key FROM exp_entries WHERE id = $1) old WHERE e.id = $1 RETURNING old.receipt_key",
    )
    .bind(id)
    .bind(&key)
    .bind(&name)
    .bind(content_type)
    .bind(size)
    .fetch_one(&mut *conn)
    .await?;
    if storage.scans_uploads() {
        huxley_db::scans::queue(
            &mut *conn,
            huxley_db::scans::NewScan {
                storage_key: &key,
                org_id: e.org_id,
                source: huxley_db::scans::Source::Receipt,
                record_id: id,
                filename: &name,
                size_bytes: size,
            },
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.receipt_added")
            .org(e.org_id)
            .target("expense", id)
            .details(json!({ "name": name, "bytes": size })),
    )
    .await?;
    sqlx::query(
        "UPDATE exp_entries SET reading = NULL, reading_error = NULL, read_at = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "expense.receipt_added",
            org_id: Some(e.org_id),
            subject_type: "expense",
            subject_id: id,
            payload: json!({
                "expenseId": id, "userId": e.user_id, "category": e.category_name, "description": e.description,
                "contentType": content_type, "link": "/expenses",
            }),
        },
    )
    .await?;
    if let Some(old) = old {
        let _ = storage.delete(&old).await;
    }
    get(conn, id).await
}

/// Removes an entry's receipt file (an administrator deleting one the
/// malware scan quarantined, ADR-094). The entry stays, without a receipt.
pub async fn drop_receipt(
    conn: &mut PgConnection,
    storage: &Storage,
    ctx: &RequestContext,
    id: Uuid,
) -> ExpenseResult<()> {
    let row: Option<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT org_id, receipt_key, receipt_name FROM exp_entries WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (org_id, Some(key), name) = row.ok_or(ExpenseError::NotFound)? else {
        return Err(ExpenseError::NotFound);
    };
    sqlx::query(
        "UPDATE exp_entries SET receipt_key = NULL, receipt_name = NULL, receipt_type = NULL, receipt_size = NULL, \
            reading = NULL, reading_error = NULL, read_at = NULL, version = version + 1, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.receipt_removed")
            .org(org_id)
            .target("expense", id)
            .details(json!({ "name": name })),
    )
    .await?;
    // Unreachable now either way, so a storage hiccup doesn't fail it.
    let _ = storage.delete(&key).await;
    Ok(())
}

/// An entry's receipt: `(file name, content type, bytes)`.
pub async fn receipt(
    conn: &mut PgConnection,
    storage: &Storage,
    id: Uuid,
) -> ExpenseResult<(String, String, Bytes)> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT receipt_key, receipt_name, receipt_type FROM exp_entries WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (Some(key), name, kind) = row.ok_or(ExpenseError::NotFound)? else {
        return Err(ExpenseError::NotFound);
    };
    let scan = huxley_db::scans::status(&mut *conn, &key).await?;
    if !scan.servable(storage.holds_unscanned()) {
        return Err(ExpenseError::Withheld(scan));
    }
    let data = storage.get(&key).await?;
    Ok((
        name.unwrap_or_else(|| "receipt".into()),
        kind.unwrap_or_else(|| "application/octet-stream".into()),
        data,
    ))
}

/// Expenses on a ticket or project, totalled for its page.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub cost: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub billable: Decimal,
}

pub fn totals(entries: &[Expense]) -> Totals {
    Totals {
        cost: entries.iter().map(|e| e.amount).sum(),
        billable: entries.iter().map(|e| e.bill_amount).sum(),
    }
}

/// A receipt to read: `(content type, bytes)`, or none if there isn't one.
pub async fn receipt_for_reading(
    conn: &mut PgConnection,
    storage: &Storage,
    id: Uuid,
) -> ExpenseResult<Option<(String, Bytes)>> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT receipt_key, receipt_type FROM exp_entries WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let (key, kind) = row.ok_or(ExpenseError::NotFound)?;
    let (Some(key), Some(kind)) = (key, kind) else {
        return Ok(None);
    };
    // A quarantined receipt isn't read (or sent to an AI provider).
    if let scan @ huxley_db::scans::Status::Infected(_) =
        huxley_db::scans::status(&mut *conn, &key).await?
    {
        return Err(ExpenseError::Withheld(scan));
    }
    Ok(Some((kind, storage.get(&key).await?)))
}

/// Keeps what was read from an entry's receipt (or why it couldn't be).
pub async fn record_reading(
    conn: &mut PgConnection,
    id: Uuid,
    reading: Result<&Reading, &str>,
) -> ExpenseResult<Expense> {
    let (value, error) = match reading {
        Ok(r) => (Some(serde_json::to_value(r).unwrap_or_default()), None),
        Err(e) => (None, Some(e.chars().take(1000).collect::<String>())),
    };
    let done = sqlx::query(
        "UPDATE exp_entries SET reading = $2, reading_error = $3, read_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(value)
    .bind(error)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ExpenseError::NotFound);
    }
    get(conn, id).await
}

/// Takes what was read from the receipt into a draft: the amount, tax,
/// currency, day and vendor it found (keeping what it didn't).
pub async fn apply_reading(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> ExpenseResult<Expense> {
    let e = editable(&mut *conn, id, None).await?;
    let r = e
        .reading
        .clone()
        .ok_or_else(|| ExpenseError::Conflict("nothing has been read from the receipt".into()))?;
    let money = |v: Option<&String>, field: &str| -> ExpenseResult<Option<Decimal>> {
        v.map(|s| {
            s.trim()
                .parse::<Decimal>()
                .map_err(|_| ExpenseError::invalid(field, "not a number"))
        })
        .transpose()
    };
    let amount = money(r.amount.as_ref(), "amount")?
        .filter(|a| *a >= Decimal::ZERO)
        .unwrap_or(e.amount);
    let tax = money(r.tax.as_ref(), "tax")?
        .filter(|t| *t >= Decimal::ZERO && *t <= amount)
        .unwrap_or(e.tax.min(amount));
    let currency = r
        .currency
        .as_deref()
        .and_then(|c| crate::currency(Some(c)).ok())
        .unwrap_or_else(|| e.currency.clone());
    let format = time::macros::format_description!("[year]-[month]-[day]");
    let day = r
        .spent_on
        .as_deref()
        .and_then(|d| Date::parse(d, format).ok())
        .filter(|d| check_day(*d).is_ok())
        .unwrap_or(e.spent_on);
    let vendor = r
        .vendor
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(200).collect::<String>())
        .or_else(|| e.vendor.clone());
    if e.kind == Kind::Mileage {
        return Err(ExpenseError::Conflict(
            "mileage has no receipt to read".into(),
        ));
    }
    sqlx::query(
        "UPDATE exp_entries SET amount = $2, tax = $3, currency = $4, spent_on = $5, vendor = $6, version = version + 1, \
            updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(amount)
    .bind(tax)
    .bind(&currency)
    .bind(day)
    .bind(&vendor)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense.reading_applied")
            .org(e.org_id)
            .target("expense", id)
            .details(json!({ "amount": amount.normalize().to_string(), "vendor": vendor })),
    )
    .await?;
    get(conn, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_are_photos_or_pdfs() {
        assert!(receipt_type("image/jpeg"));
        assert!(receipt_type("application/pdf"));
        assert!(!receipt_type("text/html"));
        assert!(!receipt_type("image/svg+xml"));
    }
}
