//! Billing settings per provider: the default hourly rate (for time no
//! agreement prices), currency, payment terms, and whether only approved
//! timesheets are billed. A client uses its nearest provider's.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{BillingError, BillingResult};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = BillingSettings)]
pub struct Settings {
    #[serde(
        serialize_with = "crate::plain_opt",
        deserialize_with = "crate::number_opt",
        default
    )]
    #[schema(value_type = Option<String>)]
    pub hourly_rate: Option<Decimal>,
    pub currency: String,
    pub payment_days: i32,
    pub approved_time_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hourly_rate: None,
            currency: "USD".into(),
            payment_days: 30,
            approved_time_only: false,
        }
    }
}

/// An organization's own settings (defaults if none are kept).
pub async fn get(conn: &mut PgConnection, org: Uuid) -> BillingResult<Settings> {
    Ok(sqlx::query_as(
        "SELECT hourly_rate, currency, payment_days, approved_time_only FROM billing_settings WHERE org_id = $1",
    )
    .bind(org)
    .fetch_optional(conn)
    .await?
    .unwrap_or_default())
}

/// The settings that apply to a client: its nearest ancestor's (or its own).
pub async fn for_client(conn: &mut PgConnection, client: Uuid) -> BillingResult<Settings> {
    Ok(sqlx::query_as(
        "SELECT b.hourly_rate, b.currency, b.payment_days, b.approved_time_only FROM billing_settings b \
           JOIN organizations bo ON bo.id = b.org_id JOIN organizations c ON c.id = $1 \
          WHERE bo.path @> c.path ORDER BY nlevel(bo.path) DESC LIMIT 1",
    )
    .bind(client)
    .fetch_optional(conn)
    .await?
    .unwrap_or_default())
}

pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org: Uuid,
    s: &Settings,
) -> BillingResult<Settings> {
    let currency = s.currency.trim().to_ascii_uppercase();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(BillingError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ));
    }
    if !(0..=365).contains(&s.payment_days) {
        return Err(BillingError::invalid("paymentDays", "0 to 365"));
    }
    if s.hourly_rate.is_some_and(|r| r < Decimal::ZERO) {
        return Err(BillingError::invalid("hourlyRate", "can't be negative"));
    }
    sqlx::query(
        "INSERT INTO billing_settings (org_id, hourly_rate, currency, payment_days, approved_time_only) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (org_id) DO UPDATE SET hourly_rate = $2, currency = $3, \
            payment_days = $4, approved_time_only = $5",
    )
    .bind(org)
    .bind(s.hourly_rate)
    .bind(&currency)
    .bind(s.payment_days)
    .bind(s.approved_time_only)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("billing.settings_updated").org(org).details(json!({
            "hourlyRate": s.hourly_rate.map(|r| r.normalize().to_string()), "currency": currency,
            "paymentDays": s.payment_days, "approvedTimeOnly": s.approved_time_only,
        })),
    )
    .await?;
    get(conn, org).await
}
