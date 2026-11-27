//! Vendors an organization buys from.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{InventoryError, InventoryResult, required, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Vendor {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub account_number: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub website: Option<String>,
    pub address: Option<String>,
    pub currency: String,
    pub payment_terms: Option<String>,
    pub notes: Option<String>,
    pub active: bool,
    /// Purchase orders not yet received or cancelled.
    pub open_orders: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VendorInput {
    pub name: String,
    #[serde(default)]
    pub account_number: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default = "usd")]
    pub currency: String,
    #[serde(default)]
    pub payment_terms: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default = "yes")]
    pub active: bool,
}

fn usd() -> String {
    "USD".into()
}

const fn yes() -> bool {
    true
}

const SELECT: &str = "SELECT v.id, v.org_id, v.name, v.account_number, v.email, v.phone, v.website, v.address, \
        v.currency, v.payment_terms, v.notes, v.active, \
        (SELECT count(*) FROM procurement_orders o WHERE o.vendor_id = v.id \
           AND o.status NOT IN ('received', 'cancelled')) AS open_orders, v.created_at \
   FROM procurement_vendors v";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct VendorFilter {
    /// Name contains.
    pub q: Option<String>,
    /// Include vendors no longer used.
    #[serde(default)]
    pub all: bool,
}

pub async fn list(conn: &mut PgConnection, filter: &VendorFilter) -> InventoryResult<Vec<Vendor>> {
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
        "{SELECT} WHERE ($1::text IS NULL OR v.name ILIKE $1) AND ($2 OR v.active) ORDER BY v.name LIMIT 500"
    )))
    .bind(q)
    .bind(filter.all)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Vendor> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE v.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(InventoryError::NotFound)
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM procurement_vendors WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

struct Checked {
    name: String,
    account: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    website: Option<String>,
    address: Option<String>,
    currency: String,
    terms: Option<String>,
    notes: Option<String>,
}

async fn check(
    conn: &mut PgConnection,
    org_id: Uuid,
    id: Option<Uuid>,
    input: &VendorInput,
) -> InventoryResult<Checked> {
    let name = required("name", &input.name, 200)?;
    let email = text("email", input.email.as_deref(), 320)?;
    if email.as_deref().is_some_and(|e| !e.contains('@')) {
        return Err(InventoryError::invalid("email", "not an email address"));
    }
    let currency = input.currency.trim().to_ascii_uppercase();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(InventoryError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM procurement_vendors WHERE org_id = $1 AND lower(name) = lower($2) \
            AND id IS DISTINCT FROM $3)",
    )
    .bind(org_id)
    .bind(&name)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(InventoryError::invalid(
            "name",
            "there's already a vendor by that name",
        ));
    }
    Ok(Checked {
        name,
        account: text("accountNumber", input.account_number.as_deref(), 100)?,
        email,
        phone: text("phone", input.phone.as_deref(), 50)?,
        website: text("website", input.website.as_deref(), 500)?,
        address: text("address", input.address.as_deref(), 1000)?,
        currency,
        terms: text("paymentTerms", input.payment_terms.as_deref(), 200)?,
        notes: text("notes", input.notes.as_deref(), 4000)?,
    })
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &VendorInput,
) -> InventoryResult<Vendor> {
    let c = check(&mut *conn, org_id, None, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO procurement_vendors (id, org_id, name, account_number, email, phone, website, address, \
            currency, payment_terms, notes, active) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(&c.account)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.website)
    .bind(&c.address)
    .bind(&c.currency)
    .bind(&c.terms)
    .bind(&c.notes)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vendor.created")
            .org(org_id)
            .target("vendor", id)
            .details(json!({ "name": c.name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &VendorInput,
) -> InventoryResult<Vendor> {
    let current = get(&mut *conn, id).await?;
    let c = check(&mut *conn, current.org_id, Some(id), input).await?;
    sqlx::query(
        "UPDATE procurement_vendors SET name = $2, account_number = $3, email = $4, phone = $5, website = $6, \
            address = $7, currency = $8, payment_terms = $9, notes = $10, active = $11, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.account)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.website)
    .bind(&c.address)
    .bind(&c.currency)
    .bind(&c.terms)
    .bind(&c.notes)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("vendor.updated")
            .org(current.org_id)
            .target("vendor", id)
            .details(json!({ "name": c.name, "active": input.active })),
    )
    .await?;
    get(conn, id).await
}

/// Removes a vendor with no purchase orders (turn others off instead).
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> InventoryResult<()> {
    let current = get(&mut *conn, id).await?;
    let used: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM procurement_orders WHERE vendor_id = $1)")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if used {
        return Err(InventoryError::Conflict(
            "it has purchase orders; turn it off instead".into(),
        ));
    }
    sqlx::query("DELETE FROM procurement_vendors WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("vendor.deleted")
            .org(current.org_id)
            .target("vendor", id)
            .details(json!({ "name": current.name })),
    )
    .await?;
    Ok(())
}
