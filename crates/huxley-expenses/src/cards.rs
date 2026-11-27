//! Card transactions (ADR-084): company card charges, imported and matched
//! to expense entries.
//!
//! - **Importing** takes rows (from a CSV the web app reads, or the API,
//!   e.g. a bank integration's workflow); a row whose bank ID was imported
//!   before is skipped. Each new row is matched straight away when one of
//!   the cardholder's expenses has the same amount and currency within
//!   three days and no transaction yet.
//! - **By hand**: match a transaction to an expense, unmatch it, ignore it,
//!   or log it as an expense (the cardholder, for their own card).

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

use crate::{ExpenseError, ExpenseResult, required, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CardTransaction {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Option<Uuid>,
    pub user_name: Option<String>,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub posted_on: Date,
    pub description: String,
    pub merchant: Option<String>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    pub currency: String,
    pub card_last4: Option<String>,
    pub external_id: Option<String>,
    pub expense_id: Option<Uuid>,
    pub expense_description: Option<String>,
    /// `unmatched`, `matched` or `ignored`.
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT: &str = "SELECT t.id, t.org_id, t.user_id, u.display_name AS user_name, t.posted_on, t.description, \
        t.merchant, t.amount, t.currency, t.card_last4, t.external_id, t.expense_id, e.description AS expense_description, \
        t.status, t.created_at \
   FROM exp_card_transactions t LEFT JOIN users u ON u.id = t.user_id LEFT JOIN exp_entries e ON e.id = t.expense_id";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct CardFilter {
    /// `unmatched`, `matched` or `ignored`.
    pub status: Option<String>,
    /// Whose card (the API sets it to you unless you see everyone's).
    pub user_id: Option<Uuid>,
}

pub async fn list(conn: &mut PgConnection, f: &CardFilter) -> ExpenseResult<Vec<CardTransaction>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR t.status = $1) AND ($2::uuid IS NULL OR t.user_id = $2) \
          ORDER BY t.posted_on DESC, t.created_at DESC LIMIT 500"
    )))
    .bind(f.status.as_deref())
    .bind(f.user_id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<CardTransaction> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(ExpenseError::NotFound)
}

/// Whose card a transaction is and its organization: `(user, org)`.
pub async fn owner_of(
    conn: &mut PgConnection,
    id: Uuid,
) -> ExpenseResult<Option<(Option<Uuid>, Uuid)>> {
    Ok(
        sqlx::query_as("SELECT user_id, org_id FROM exp_card_transactions WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// One row to import.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardRow {
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub posted_on: Date,
    pub description: String,
    #[serde(default)]
    pub merchant: Option<String>,
    #[serde(deserialize_with = "crate::number_opt")]
    #[schema(value_type = String)]
    pub amount: Option<Decimal>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub card_last4: Option<String>,
    /// The bank's ID for it: importing it again is skipped.
    #[serde(default)]
    pub external_id: Option<String>,
    /// Whose card (default: none).
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub imported: usize,
    /// Rows imported before (by their bank ID).
    pub skipped: usize,
    /// Of the imported, how many were matched to an expense.
    pub matched: usize,
}

/// Imports rows for `org` (an MSP).
pub async fn import(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org: Uuid,
    rows: &[CardRow],
) -> ExpenseResult<Imported> {
    if rows.len() > 5000 {
        return Err(ExpenseError::invalid("rows", "at most 5,000 at a time"));
    }
    let mut out = Imported::default();
    for (i, r) in rows.iter().enumerate() {
        let f = |k: &str| format!("rows/{i}/{k}");
        let description = required(&f("description"), &r.description, 500)?;
        let merchant = text(&f("merchant"), r.merchant.as_deref(), 200)?;
        let amount = r
            .amount
            .ok_or_else(|| ExpenseError::invalid(&f("amount"), "required"))?;
        let currency = crate::currency(r.currency.as_deref())?;
        let last4 = text(&f("cardLast4"), r.card_last4.as_deref(), 4)?;
        if last4
            .as_deref()
            .is_some_and(|l| l.len() != 4 || !l.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(ExpenseError::invalid(
                &f("cardLast4"),
                "the card's last four digits",
            ));
        }
        let external = text(&f("externalId"), r.external_id.as_deref(), 200)?;
        let id = huxley_core::new_id();
        let inserted = sqlx::query(
            "INSERT INTO exp_card_transactions (id, org_id, user_id, posted_on, description, merchant, amount, currency, \
                card_last4, external_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (org_id, external_id) WHERE external_id IS NOT NULL DO NOTHING",
        )
        .bind(id)
        .bind(org)
        .bind(r.user_id)
        .bind(r.posted_on)
        .bind(&description)
        .bind(&merchant)
        .bind(amount)
        .bind(&currency)
        .bind(&last4)
        .bind(&external)
        .execute(&mut *conn)
        .await?;
        if inserted.rows_affected() == 0 {
            out.skipped += 1;
            continue;
        }
        out.imported += 1;
        if auto_match(&mut *conn, id).await? {
            out.matched += 1;
        }
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("card_transactions.imported")
            .org(org)
            .details(
                json!({ "imported": out.imported, "skipped": out.skipped, "matched": out.matched }),
            ),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "card_transactions.imported",
            org_id: Some(org),
            subject_type: "organization",
            subject_id: org,
            payload: json!({ "orgId": org, "imported": out.imported, "matched": out.matched,
                             "unmatched": out.imported - out.matched, "link": "/expenses/cards" }),
        },
    )
    .await?;
    Ok(out)
}

/// Matches a new transaction to the cardholder's expense with the same
/// amount and currency within three days, if there's exactly one free.
async fn auto_match(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<bool> {
    let found: Vec<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM exp_card_transactions t JOIN exp_entries e \
             ON e.user_id = t.user_id AND e.amount = t.amount AND e.currency = t.currency \
            AND abs(e.spent_on - t.posted_on) <= 3 \
          WHERE t.id = $1 AND NOT EXISTS (SELECT 1 FROM exp_card_transactions x WHERE x.expense_id = e.id) \
          LIMIT 2",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let [expense] = found.as_slice() else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE exp_card_transactions SET expense_id = $2, status = 'matched' WHERE id = $1",
    )
    .bind(id)
    .bind(expense)
    .execute(conn)
    .await?;
    Ok(true)
}

/// Matches a transaction to an expense (of its cardholder, when it has one).
pub async fn match_to(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    expense: Uuid,
) -> ExpenseResult<CardTransaction> {
    let t = get(&mut *conn, id).await?;
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM exp_entries WHERE id = $1")
        .bind(expense)
        .fetch_optional(&mut *conn)
        .await?;
    let owner = owner.ok_or_else(|| ExpenseError::invalid("expenseId", "no such expense"))?;
    if t.user_id.is_some_and(|u| u != owner) {
        return Err(ExpenseError::invalid(
            "expenseId",
            "that's someone else's expense",
        ));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM exp_card_transactions WHERE expense_id = $1 AND id <> $2)",
    )
    .bind(expense)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(ExpenseError::Conflict(
            "that expense is matched to another transaction".into(),
        ));
    }
    sqlx::query(
        "UPDATE exp_card_transactions SET expense_id = $2, status = 'matched' WHERE id = $1",
    )
    .bind(id)
    .bind(expense)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("card_transaction.matched")
            .org(t.org_id)
            .target("card_transaction", id)
            .details(json!({ "expense": expense })),
    )
    .await?;
    get(conn, id).await
}

/// Sets a transaction unmatched or ignored.
pub async fn set_status(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    ignored: bool,
) -> ExpenseResult<CardTransaction> {
    let t = get(&mut *conn, id).await?;
    let status = if ignored { "ignored" } else { "unmatched" };
    sqlx::query("UPDATE exp_card_transactions SET expense_id = NULL, status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if ignored {
            "card_transaction.ignored"
        } else {
            "card_transaction.unmatched"
        })
        .org(t.org_id)
        .target("card_transaction", id),
    )
    .await?;
    get(conn, id).await
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogInput {
    pub category_id: Uuid,
    /// Default: the transaction's description.
    #[serde(default)]
    pub description: Option<String>,
    /// Where it goes: a ticket, project, client or the MSP (default: the MSP).
    #[serde(default)]
    pub ticket_id: Option<Uuid>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub org_id: Option<Uuid>,
}

/// Logs a transaction on your card as your expense, and matches them.
pub async fn log_as_expense(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    id: Uuid,
    input: &LogInput,
) -> ExpenseResult<CardTransaction> {
    let t = get(&mut *conn, id).await?;
    if t.user_id != Some(user) {
        return Err(ExpenseError::Conflict(
            "only the cardholder logs it as their expense".into(),
        ));
    }
    if t.status != "unmatched" {
        return Err(ExpenseError::Conflict(
            "it's already matched or ignored".into(),
        ));
    }
    let entry: crate::entries::ExpenseInput = serde_json::from_value(json!({
        "categoryId": input.category_id,
        "spentOn": t.posted_on.to_string(),
        "description": input.description.clone().unwrap_or_else(|| t.description.clone()),
        "vendor": t.merchant,
        "amount": t.amount.abs().normalize().to_string(),
        "currency": t.currency,
        "ticketId": input.ticket_id,
        "projectId": input.project_id,
        "orgId": input.org_id.or(Some(t.org_id)),
        // Paid with the company's card: nothing to pay back.
        "reimbursable": false,
    }))
    .map_err(|e| ExpenseError::invalid("input", e.to_string()))?;
    let e = crate::entries::create(&mut *conn, ctx, user, &entry).await?;
    match_to(conn, ctx, id, e.id).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn rows_need_an_amount() {
        let row: Result<super::CardRow, _> = serde_json::from_value(
            serde_json::json!({ "postedOn": "2026-11-01", "description": "x" }),
        );
        assert!(row.is_err());
    }
}
