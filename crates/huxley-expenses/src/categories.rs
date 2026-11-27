//! Expense categories: instance-wide, with defaults and policy.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ExpenseError, ExpenseResult, required};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ExpenseCategoryKind)]
pub enum Kind {
    Expense,
    /// A distance at the mileage rate.
    Mileage,
}

#[allow(clippy::struct_excessive_bools)] // independent settings, one per column
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ExpenseCategory)]
pub struct Category {
    pub id: Uuid,
    pub name: String,
    pub kind: Kind,
    pub billable: bool,
    pub reimbursable: bool,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub markup_pct: Decimal,
    /// Entries over this are flagged.
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub limit_amount: Option<Decimal>,
    pub receipt_required: bool,
    pub active: bool,
}

#[allow(clippy::struct_excessive_bools)] // independent settings, one per column
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ExpenseCategoryInput)]
pub struct CategoryInput {
    pub name: String,
    #[serde(default = "expense")]
    pub kind: Kind,
    #[serde(default)]
    pub billable: bool,
    #[serde(default = "yes")]
    pub reimbursable: bool,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub markup_pct: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::number_opt")]
    #[schema(value_type = Option<String>)]
    pub limit_amount: Option<Decimal>,
    #[serde(default = "yes")]
    pub receipt_required: bool,
    #[serde(default = "yes")]
    pub active: bool,
}

const fn yes() -> bool {
    true
}

const fn expense() -> Kind {
    Kind::Expense
}

const SELECT: &str = "SELECT id, name, kind, billable, reimbursable, markup_pct, limit_amount, receipt_required, active \
    FROM exp_categories";

pub async fn list(conn: &mut PgConnection) -> ExpenseResult<Vec<Category>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY position, name"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ExpenseResult<Category> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(ExpenseError::NotFound)
}

fn check(input: &CategoryInput) -> ExpenseResult<String> {
    let name = required("name", &input.name, 100)?;
    if input.markup_pct.is_some_and(|m| m < Decimal::ZERO) {
        return Err(ExpenseError::invalid("markupPct", "zero or more"));
    }
    if input.limit_amount.is_some_and(|l| l < Decimal::ZERO) {
        return Err(ExpenseError::invalid("limitAmount", "zero or more"));
    }
    Ok(name)
}

fn taken(e: sqlx::Error) -> ExpenseError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            ExpenseError::invalid("name", "a category with this name already exists")
        }
        _ => ExpenseError::Sqlx(e),
    }
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &CategoryInput,
) -> ExpenseResult<Category> {
    let name = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO exp_categories (id, name, kind, billable, reimbursable, markup_pct, limit_amount, receipt_required, \
            active, position) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, (SELECT coalesce(max(position), 0) + 1 FROM exp_categories))",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.billable)
    .bind(input.reimbursable)
    .bind(input.markup_pct.unwrap_or_default())
    .bind(input.limit_amount)
    .bind(input.receipt_required)
    .bind(input.active)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_category.created")
            .target("expense_category", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &CategoryInput,
) -> ExpenseResult<Category> {
    let name = check(input)?;
    let done = sqlx::query(
        "UPDATE exp_categories SET name = $2, kind = $3, billable = $4, reimbursable = $5, markup_pct = $6, \
            limit_amount = $7, receipt_required = $8, active = $9 WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(input.kind)
    .bind(input.billable)
    .bind(input.reimbursable)
    .bind(input.markup_pct.unwrap_or_default())
    .bind(input.limit_amount)
    .bind(input.receipt_required)
    .bind(input.active)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    if done.rows_affected() == 0 {
        return Err(ExpenseError::NotFound);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("expense_category.updated")
            .target("expense_category", id)
            .details(json!({ "name": name, "active": input.active })),
    )
    .await?;
    get(conn, id).await
}
