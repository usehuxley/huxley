//! Forecasting (ADR-071): won deals and the open pipeline per salesperson
//! for a period, by forecast category, against their quota. Values are
//! first-year: one-time plus twelve months of recurring.
//!
//! - **Won**: deals won with their close in the period.
//! - **Commit**, **best case**, **pipeline**: open deals expected to close
//!   in the period, by forecast category (best case includes commit, and
//!   pipeline includes both). Deals left out of the forecast aren't counted.
//! - **Weighted**: the open deals' value times their probability.
//! - **Quota**: the quotas starting in the period (set monthly or
//!   quarterly, not both).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::Date;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{SalesError, SalesResult};

/// One salesperson's forecast (in one currency).
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ForecastRow)]
pub struct Row {
    /// None: deals without an owner.
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub currency: String,
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub quota: Option<Decimal>,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub won: Decimal,
    pub won_count: i64,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub commit: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub best_case: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub pipeline: Decimal,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub weighted: Decimal,
    pub open_count: i64,
    /// Won as a percentage of quota.
    #[serde(serialize_with = "crate::plain_opt")]
    #[schema(value_type = Option<String>)]
    pub attainment: Option<Decimal>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Period {
    /// The first day (`YYYY-MM-DD`).
    #[serde(with = "crate::date::required")]
    #[param(value_type = String, format = Date)]
    pub from: Date,
    /// The last day, included.
    #[serde(with = "crate::date::required")]
    #[param(value_type = String, format = Date)]
    pub to: Date,
}

/// The forecast for a period, per owner and currency (in the caller's scope).
pub async fn forecast(conn: &mut PgConnection, period: &Period) -> SalesResult<Vec<Row>> {
    if period.to < period.from {
        return Err(SalesError::invalid("to", "ends before it starts"));
    }
    Ok(sqlx::query_as(
        "WITH deals AS ( \
           SELECT op.owner_id, op.currency, op.status, op.forecast, \
                  op.amount + 12 * op.recurring AS value, op.probability, \
                  op.closed_at::date AS closed_on, op.close_date \
             FROM sales_opportunities op), \
         q AS ( \
           SELECT user_id, currency, sum(amount) AS quota FROM sales_quotas \
            WHERE period_start BETWEEN $1 AND $2 \
            GROUP BY user_id, currency), \
         f AS ( \
           SELECT owner_id, currency, \
                  coalesce(sum(value) FILTER (WHERE status = 'won' AND closed_on BETWEEN $1 AND $2), 0) AS won, \
                  count(*) FILTER (WHERE status = 'won' AND closed_on BETWEEN $1 AND $2) AS won_count, \
                  coalesce(sum(value) FILTER (WHERE status = 'open' AND close_date BETWEEN $1 AND $2 \
                                                AND forecast = 'commit'), 0) AS commit, \
                  coalesce(sum(value) FILTER (WHERE status = 'open' AND close_date BETWEEN $1 AND $2 \
                                                AND forecast IN ('commit', 'best_case')), 0) AS best_case, \
                  coalesce(sum(value) FILTER (WHERE status = 'open' AND close_date BETWEEN $1 AND $2 \
                                                AND forecast <> 'omitted'), 0) AS pipeline, \
                  coalesce(sum(value * probability / 100.0) FILTER (WHERE status = 'open' \
                                                AND close_date BETWEEN $1 AND $2 AND forecast <> 'omitted'), 0) AS weighted, \
                  count(*) FILTER (WHERE status = 'open' AND close_date BETWEEN $1 AND $2 AND forecast <> 'omitted') AS open_count \
             FROM deals GROUP BY owner_id, currency) \
         SELECT coalesce(f.owner_id, q.user_id) AS owner_id, u.display_name AS owner_name, \
                coalesce(f.currency, q.currency) AS currency, q.quota, \
                coalesce(f.won, 0) AS won, coalesce(f.won_count, 0) AS won_count, \
                coalesce(f.commit, 0) AS commit, coalesce(f.best_case, 0) AS best_case, \
                coalesce(f.pipeline, 0) AS pipeline, round(coalesce(f.weighted, 0), 2) AS weighted, \
                coalesce(f.open_count, 0) AS open_count, \
                CASE WHEN q.quota > 0 THEN round(coalesce(f.won, 0) * 100 / q.quota, 1) END AS attainment \
           FROM f FULL JOIN q ON q.user_id = f.owner_id AND q.currency = f.currency \
           LEFT JOIN users u ON u.id = coalesce(f.owner_id, q.user_id) \
          WHERE coalesce(f.won_count, 0) + coalesce(f.open_count, 0) > 0 OR q.quota IS NOT NULL \
          ORDER BY u.display_name NULLS LAST, currency",
    )
    .bind(period.from)
    .bind(period.to)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesQuota)]
pub struct Quota {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub user_name: Option<String>,
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub period_start: Date,
    pub months: i32,
    #[serde(serialize_with = "crate::plain")]
    #[schema(value_type = String)]
    pub amount: Decimal,
    pub currency: String,
}

/// Quotas starting in a year.
pub async fn quotas(conn: &mut PgConnection, year: i32) -> SalesResult<Vec<Quota>> {
    Ok(sqlx::query_as(
        "SELECT q.id, q.org_id, q.user_id, u.display_name AS user_name, q.period_start, q.months, q.amount, q.currency \
           FROM sales_quotas q LEFT JOIN users u ON u.id = q.user_id \
          WHERE extract(year FROM q.period_start) = $1 ORDER BY u.display_name, q.period_start, q.months",
    )
    .bind(year)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SalesQuotaInput)]
pub struct QuotaInput {
    /// The organization they sell for.
    pub org_id: Uuid,
    pub user_id: Uuid,
    /// The first day of a month (or of a quarter, for 3 months).
    #[serde(with = "crate::date::required")]
    #[schema(value_type = String, format = Date)]
    pub period_start: Date,
    /// 1 (a month) or 3 (a quarter).
    pub months: i32,
    /// First-year value; zero or empty removes it.
    #[serde(default, deserialize_with = "crate::money_opt")]
    #[schema(value_type = Option<String>)]
    pub amount: Option<Decimal>,
    #[serde(default = "usd")]
    pub currency: String,
}

fn usd() -> String {
    "USD".into()
}

/// Sets (or, with no amount, removes) a quota.
pub async fn set_quota(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &QuotaInput,
) -> SalesResult<()> {
    if input.period_start.day() != 1 {
        return Err(SalesError::invalid(
            "periodStart",
            "the first day of a month",
        ));
    }
    if input.months != 1 && input.months != 3 {
        return Err(SalesError::invalid(
            "months",
            "1 (a month) or 3 (a quarter)",
        ));
    }
    if input.months == 3 && !matches!(u8::from(input.period_start.month()), 1 | 4 | 7 | 10) {
        return Err(SalesError::invalid(
            "periodStart",
            "a quarter starts in January, April, July or October",
        ));
    }
    let currency = crate::currency(&input.currency)?;
    match input.amount.filter(|a| *a > Decimal::ZERO) {
        Some(amount) => {
            sqlx::query(
                "INSERT INTO sales_quotas (id, org_id, user_id, period_start, months, amount, currency) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (user_id, period_start, months) \
                 DO UPDATE SET amount = $6, currency = $7, org_id = $2, updated_at = now()",
            )
            .bind(huxley_core::new_id())
            .bind(input.org_id)
            .bind(input.user_id)
            .bind(input.period_start)
            .bind(input.months)
            .bind(amount)
            .bind(&currency)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM sales_quotas WHERE user_id = $1 AND period_start = $2 AND months = $3",
            )
            .bind(input.user_id)
            .bind(input.period_start)
            .bind(input.months)
            .execute(&mut *conn)
            .await?;
        }
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sales_quota.set").org(input.org_id).details(json!({
            "user": input.user_id, "periodStart": input.period_start.to_string(), "months": input.months,
            "amount": input.amount.map(|a| a.normalize().to_string()),
        })),
    )
    .await?;
    Ok(())
}
