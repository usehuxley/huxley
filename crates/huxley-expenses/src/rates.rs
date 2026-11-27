//! Mileage rates: the one in effect on the day is used.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ExpenseError, ExpenseResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MileageRate {
    pub id: Uuid,
    /// `mi` or `km`.
    pub unit: String,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub rate: Decimal,
    pub currency: String,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub effective_from: Date,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MileageRateInput {
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(deserialize_with = "crate::number_opt")]
    #[schema(value_type = String)]
    pub rate: Option<Decimal>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub effective_from: Date,
}

pub async fn list(conn: &mut PgConnection) -> ExpenseResult<Vec<MileageRate>> {
    Ok(sqlx::query_as(
        "SELECT id, unit, rate, currency, effective_from FROM exp_mileage_rates ORDER BY unit, currency, effective_from DESC",
    )
    .fetch_all(conn)
    .await?)
}

pub(crate) fn unit(value: Option<&str>) -> ExpenseResult<String> {
    match value.map(str::trim) {
        None | Some("" | "mi") => Ok("mi".into()),
        Some("km") => Ok("km".into()),
        Some(_) => Err(ExpenseError::invalid("unit", "mi or km")),
    }
}

/// Sets (or replaces) the rate from a day.
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &MileageRateInput,
) -> ExpenseResult<MileageRate> {
    let unit = unit(input.unit.as_deref())?;
    let currency = crate::currency(input.currency.as_deref())?;
    let rate = input
        .rate
        .filter(|r| *r >= Decimal::ZERO)
        .ok_or_else(|| ExpenseError::invalid("rate", "zero or more"))?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO exp_mileage_rates (id, unit, rate, currency, effective_from) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (unit, currency, effective_from) DO UPDATE SET rate = $3 RETURNING id",
    )
    .bind(huxley_core::new_id())
    .bind(&unit)
    .bind(rate)
    .bind(&currency)
    .bind(input.effective_from)
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("mileage_rate.set")
            .target("mileage_rate", id)
            .details(
                json!({ "unit": unit, "rate": rate.normalize().to_string(), "currency": currency,
                             "from": input.effective_from.to_string() }),
            ),
    )
    .await?;
    Ok(sqlx::query_as(
        "SELECT id, unit, rate, currency, effective_from FROM exp_mileage_rates WHERE id = $1",
    )
    .bind(id)
    .fetch_one(conn)
    .await?)
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> ExpenseResult<()> {
    let done = sqlx::query("DELETE FROM exp_mileage_rates WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Err(ExpenseError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("mileage_rate.deleted").target("mileage_rate", id),
    )
    .await?;
    Ok(())
}

/// The rate in effect on `day` for `unit` in `currency`.
pub async fn on(
    conn: &mut PgConnection,
    unit: &str,
    currency: &str,
    day: Date,
) -> ExpenseResult<Option<Decimal>> {
    Ok(sqlx::query_scalar(
        "SELECT rate FROM exp_mileage_rates WHERE unit = $1 AND currency = $2 AND effective_from <= $3 \
          ORDER BY effective_from DESC LIMIT 1",
    )
    .bind(unit)
    .bind(currency)
    .bind(day)
    .fetch_optional(conn)
    .await?)
}
