//! Agreements (ADR-039): what a client has agreed to. A kind and a term,
//! coverage (work types, asset types, an allowance), a rate card, and
//! recurring billing lines.

use std::collections::HashMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::catalog::{decimal, money, plain, plain_one, quantity};
use crate::{ContractsError, ContractsResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AgreementKind {
    /// A fee covers some kinds of work, maybe up to some hours a month.
    Managed,
    /// Hours bought up front, drawn down by covered work.
    BlockHours,
    /// Billable work at the agreed rates.
    TimeMaterials,
    /// Like time and materials, for a piece of work.
    Project,
}

impl AgreementKind {
    /// Whether it has coverage (rather than billing everything).
    pub fn covers(self) -> bool {
        matches!(self, Self::Managed | Self::BlockHours)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum AgreementStatus {
    Draft,
    Active,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = AgreementLineBilling)]
pub enum LineBilling {
    Monthly,
    Quarterly,
    Annual,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AgreementRate)]
pub struct Rate {
    #[serde(skip)]
    pub agreement_id: Uuid,
    /// None: the default rate.
    pub work_type_id: Option<Uuid>,
    pub work_type_name: Option<String>,
    #[serde(serialize_with = "plain_one")]
    #[schema(value_type = String)]
    pub hourly_rate: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AgreementLine)]
pub struct Line {
    pub id: Uuid,
    #[serde(skip)]
    pub agreement_id: Uuid,
    pub product_id: Option<Uuid>,
    pub product_sku: Option<String>,
    pub description: String,
    #[serde(serialize_with = "plain_one")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    /// Its own price (none: the product's).
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub unit_price: Option<Decimal>,
    /// The price it bills at: its own, else the product's.
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub effective_price: Option<Decimal>,
    pub billing: LineBilling,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Agreement {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub name: String,
    pub kind: AgreementKind,
    pub status: AgreementStatus,
    #[serde(with = "crate::date")]
    #[schema(value_type = String, example = "2026-01-01")]
    pub starts_on: Date,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>)]
    pub ends_on: Option<Date>,
    pub auto_renew: bool,
    pub renewal_notice_days: i32,
    pub currency: String,
    pub priority: i32,
    /// Coverage: every work type, or `workTypeIds`.
    pub covers_all_work_types: bool,
    #[sqlx(skip)]
    pub work_type_ids: Vec<Uuid>,
    /// Coverage: tickets about assets of these types (none: any ticket).
    #[sqlx(skip)]
    pub asset_type_ids: Vec<Uuid>,
    /// Managed: minutes included a calendar month (none: unlimited).
    pub monthly_minutes: Option<i32>,
    /// Block hours: minutes bought for the term.
    pub block_minutes: Option<i32>,
    /// Billable minutes round up to this.
    pub billing_increment: i32,
    pub notes: String,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[sqlx(skip)]
    pub rates: Vec<Rate>,
    #[sqlx(skip)]
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AgreementRateInput)]
pub struct RateInput {
    /// None: the default rate.
    #[serde(default)]
    pub work_type_id: Option<Uuid>,
    #[serde(deserialize_with = "quantity")]
    #[schema(value_type = String)]
    pub hourly_rate: Decimal,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AgreementLineInput)]
pub struct LineInput {
    #[serde(default)]
    pub product_id: Option<Uuid>,
    /// Default: the product's name.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(deserialize_with = "quantity")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(default, deserialize_with = "decimal")]
    #[schema(value_type = Option<String>)]
    pub unit_price: Option<Decimal>,
    /// Default monthly.
    #[serde(default)]
    pub billing: Option<LineBilling>,
}

fn yes() -> bool {
    true
}

fn thirty() -> i32 {
    30
}

fn one() -> i32 {
    1
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgreementInput {
    pub name: String,
    pub kind: AgreementKind,
    /// Default draft.
    #[serde(default)]
    pub status: Option<AgreementStatus>,
    #[serde(with = "crate::date")]
    #[schema(value_type = String, example = "2026-01-01")]
    pub starts_on: Date,
    #[serde(default, with = "crate::date::option")]
    #[schema(value_type = Option<String>)]
    pub ends_on: Option<Date>,
    #[serde(default)]
    pub auto_renew: bool,
    #[serde(default = "thirty")]
    pub renewal_notice_days: i32,
    /// Default USD.
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "yes")]
    pub covers_all_work_types: bool,
    #[serde(default)]
    pub work_type_ids: Vec<Uuid>,
    #[serde(default)]
    pub asset_type_ids: Vec<Uuid>,
    #[serde(default)]
    pub monthly_minutes: Option<i32>,
    #[serde(default)]
    pub block_minutes: Option<i32>,
    #[serde(default = "one")]
    pub billing_increment: i32,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub rates: Vec<RateInput>,
    #[serde(default)]
    pub lines: Vec<LineInput>,
    /// The version you read (changes only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT a.id, a.org_id, o.name AS org_name, a.name, a.kind, a.status, a.starts_on, \
    a.ends_on, a.auto_renew, a.renewal_notice_days, a.currency, a.priority, a.covers_all_work_types, \
    a.monthly_minutes, a.block_minutes, a.billing_increment, a.notes, a.version, a.created_at, a.updated_at \
    FROM contract_agreements a JOIN organizations o ON o.id = a.org_id";

/// Agreements here (RLS), at `org_id` if given, newest term first.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Option<Uuid>,
) -> ContractsResult<Vec<Agreement>> {
    let mut all: Vec<Agreement> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR a.org_id = $1) \
         ORDER BY o.name, a.status = 'cancelled', a.starts_on DESC, a.name"
    )))
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    with_details(conn, &mut all).await?;
    Ok(all)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ContractsResult<Agreement> {
    let found: Agreement = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(ContractsError::NotFound)?;
    let mut one = vec![found];
    with_details(conn, &mut one).await?;
    Ok(one.remove(0))
}

async fn with_details(
    conn: &mut PgConnection,
    agreements: &mut [Agreement],
) -> ContractsResult<()> {
    if agreements.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = agreements.iter().map(|a| a.id).collect();
    let work_types: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT agreement_id, work_type_id FROM contract_agreement_work_types WHERE agreement_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let asset_types: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT agreement_id, asset_type_id FROM contract_agreement_asset_types WHERE agreement_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let rates: Vec<Rate> = sqlx::query_as(
        "SELECT r.agreement_id, r.work_type_id, w.name AS work_type_name, r.hourly_rate \
           FROM contract_rates r LEFT JOIN sd_work_types w ON w.id = r.work_type_id \
          WHERE r.agreement_id = ANY($1) ORDER BY r.work_type_id IS NOT NULL, w.name",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let lines: Vec<Line> = sqlx::query_as(
        "SELECT l.id, l.agreement_id, l.product_id, p.sku AS product_sku, l.description, l.quantity, \
                l.unit_price, COALESCE(l.unit_price, p.price) AS effective_price, l.billing \
           FROM contract_lines l LEFT JOIN catalog_products p ON p.id = l.product_id \
          WHERE l.agreement_id = ANY($1) ORDER BY l.position, l.id",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut by: HashMap<Uuid, &mut Agreement> = agreements.iter_mut().map(|a| (a.id, a)).collect();
    for (a, w) in work_types {
        if let Some(a) = by.get_mut(&a) {
            a.work_type_ids.push(w);
        }
    }
    for (a, t) in asset_types {
        if let Some(a) = by.get_mut(&a) {
            a.asset_type_ids.push(t);
        }
    }
    for r in rates {
        if let Some(a) = by.get_mut(&r.agreement_id) {
            a.rates.push(r);
        }
    }
    for l in lines {
        if let Some(a) = by.get_mut(&l.agreement_id) {
            a.lines.push(l);
        }
    }
    Ok(())
}

/// A checked input.
struct Clean {
    name: String,
    status: AgreementStatus,
    currency: String,
    monthly_minutes: Option<i32>,
    block_minutes: Option<i32>,
    covers_all: bool,
    rates: Vec<(Option<Uuid>, Decimal)>,
}

fn check_lines(lines: &[LineInput]) -> ContractsResult<()> {
    let bad = ContractsError::invalid;
    for (i, l) in lines.iter().enumerate() {
        if l.quantity <= Decimal::ZERO || l.quantity.round_dp(4) != l.quantity {
            return Err(bad(
                &format!("lines/{i}/quantity"),
                "more than 0, with at most 4 decimal places",
            ));
        }
        money(&format!("lines/{i}/unitPrice"), l.unit_price)?;
        if l.product_id.is_none() && l.description.as_deref().is_none_or(|d| d.trim().is_empty()) {
            return Err(bad(
                &format!("lines/{i}/description"),
                "describe it, or choose a product",
            ));
        }
        if l.description
            .as_deref()
            .is_some_and(|d| d.chars().count() > 500)
        {
            return Err(bad(
                &format!("lines/{i}/description"),
                "at most 500 characters",
            ));
        }
    }
    Ok(())
}

fn check(input: &AgreementInput) -> ContractsResult<Clean> {
    let bad = ContractsError::invalid;
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(bad("name", "1 to 200 characters"));
    }
    if input.ends_on.is_some_and(|end| end < input.starts_on) {
        return Err(bad("endsOn", "on or after the start"));
    }
    if !(0..=365).contains(&input.renewal_notice_days) {
        return Err(bad("renewalNoticeDays", "0 to 365"));
    }
    if !(0..=1000).contains(&input.priority) {
        return Err(bad("priority", "0 to 1000"));
    }
    if !(1..=60).contains(&input.billing_increment) {
        return Err(bad("billingIncrement", "1 to 60 minutes"));
    }
    if input.notes.chars().count() > 4000 {
        return Err(bad("notes", "at most 4000 characters"));
    }
    let currency = input
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("USD")
        .to_ascii_uppercase();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(bad("currency", "a three-letter code, e.g. USD"));
    }
    let positive = |field: &str, v: Option<i32>| match v {
        Some(m) if m <= 0 => Err(bad(field, "more than 0")),
        other => Ok(other),
    };
    let monthly_minutes = positive("monthlyMinutes", input.monthly_minutes)?;
    let block_minutes = positive("blockMinutes", input.block_minutes)?;
    match input.kind {
        AgreementKind::BlockHours if block_minutes.is_none() => {
            return Err(bad("blockMinutes", "how many hours the block holds"));
        }
        AgreementKind::BlockHours if monthly_minutes.is_some() => {
            return Err(bad(
                "monthlyMinutes",
                "block hours have a block, not a monthly allowance",
            ));
        }
        AgreementKind::Managed if block_minutes.is_some() => {
            return Err(bad(
                "blockMinutes",
                "only block hours agreements have a block",
            ));
        }
        AgreementKind::TimeMaterials | AgreementKind::Project => {
            if monthly_minutes.is_some() || block_minutes.is_some() {
                return Err(bad(
                    "kind",
                    "time and materials and project agreements have no allowance",
                ));
            }
            if !input.work_type_ids.is_empty() || !input.asset_type_ids.is_empty() {
                return Err(bad(
                    "kind",
                    "time and materials and project agreements have no coverage",
                ));
            }
        }
        _ => {}
    }
    let covers_all = input.covers_all_work_types || !input.kind.covers();
    if input.kind.covers() && !covers_all && input.work_type_ids.is_empty() {
        return Err(bad(
            "workTypeIds",
            "choose the work types it covers, or cover them all",
        ));
    }
    let mut rates = Vec::new();
    for (i, r) in input.rates.iter().enumerate() {
        let rate =
            money(&format!("rates/{i}/hourlyRate"), Some(r.hourly_rate))?.unwrap_or_default();
        if rates.iter().any(|(w, _)| *w == r.work_type_id) {
            return Err(bad(&format!("rates/{i}/workTypeId"), "already has a rate"));
        }
        rates.push((r.work_type_id, rate));
    }
    check_lines(&input.lines)?;
    Ok(Clean {
        name,
        status: input.status.unwrap_or(AgreementStatus::Draft),
        currency,
        monthly_minutes,
        block_minutes,
        covers_all,
        rates,
    })
}

/// Checks what the input refers to: work types and asset types exist, and
/// line products are in the catalog at or above the agreement's
/// organization, priced in its currency. The lines' descriptions.
async fn check_refs(
    conn: &mut PgConnection,
    org_id: Uuid,
    input: &AgreementInput,
    currency: &str,
) -> ContractsResult<Vec<String>> {
    let bad = ContractsError::invalid;
    let mut work_types: Vec<Uuid> = input.work_type_ids.clone();
    work_types.extend(input.rates.iter().filter_map(|r| r.work_type_id));
    for (i, w) in work_types.iter().enumerate() {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sd_work_types WHERE id = $1)")
                .bind(w)
                .fetch_one(&mut *conn)
                .await?;
        if !exists {
            let field = if i < input.work_type_ids.len() {
                format!("workTypeIds/{i}")
            } else {
                "rates".to_owned()
            };
            return Err(bad(&field, "no such work type"));
        }
    }
    for (i, t) in input.asset_type_ids.iter().enumerate() {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM cmdb_asset_types WHERE id = $1)")
                .bind(t)
                .fetch_one(&mut *conn)
                .await?;
        if !exists {
            return Err(bad(&format!("assetTypeIds/{i}"), "no such asset type"));
        }
    }
    let mut descriptions = Vec::new();
    for (i, l) in input.lines.iter().enumerate() {
        let description = l
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());
        let Some(product) = l.product_id else {
            descriptions.push(description.unwrap_or_default().to_owned());
            continue;
        };
        let found: Option<(String, String, bool)> = sqlx::query_as(
            "SELECT p.name, p.currency, \
                    (SELECT path FROM organizations WHERE id = $2) <@ o.path AS above \
               FROM catalog_products p JOIN organizations o ON o.id = p.org_id WHERE p.id = $1",
        )
        .bind(product)
        .bind(org_id)
        .fetch_optional(&mut *conn)
        .await?;
        let field = format!("lines/{i}/productId");
        match found {
            None => return Err(bad(&field, "no such product")),
            Some((_, _, false)) => {
                return Err(bad(
                    &field,
                    "a product of this organization's catalog or one above it",
                ));
            }
            Some((_, c, _)) if c != currency => {
                return Err(ContractsError::invalid(
                    &field,
                    format!("it's priced in {c}, and the agreement in {currency}"),
                ));
            }
            Some((name, _, _)) => descriptions.push(description.map_or(name, str::to_owned)),
        }
    }
    Ok(descriptions)
}

/// Replaces an agreement's coverage sets, rates and lines.
async fn set_details(
    conn: &mut PgConnection,
    id: Uuid,
    org_id: Uuid,
    input: &AgreementInput,
    clean: &Clean,
    descriptions: &[String],
) -> ContractsResult<()> {
    for table in [
        "contract_agreement_work_types",
        "contract_agreement_asset_types",
        "contract_rates",
        "contract_lines",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE agreement_id = $1"
        )))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    if !clean.covers_all {
        for w in &input.work_type_ids {
            sqlx::query(
                "INSERT INTO contract_agreement_work_types (agreement_id, work_type_id, org_id) \
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(w)
            .bind(org_id)
            .execute(&mut *conn)
            .await?;
        }
    }
    for t in &input.asset_type_ids {
        sqlx::query(
            "INSERT INTO contract_agreement_asset_types (agreement_id, asset_type_id, org_id) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(t)
        .bind(org_id)
        .execute(&mut *conn)
        .await?;
    }
    for (work_type, rate) in &clean.rates {
        sqlx::query(
            "INSERT INTO contract_rates (agreement_id, work_type_id, org_id, hourly_rate) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(work_type)
        .bind(org_id)
        .bind(rate)
        .execute(&mut *conn)
        .await?;
    }
    for (i, (l, description)) in input.lines.iter().zip(descriptions).enumerate() {
        sqlx::query(
            "INSERT INTO contract_lines (id, agreement_id, org_id, product_id, description, quantity, \
                unit_price, billing, position) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(huxley_core::new_id())
        .bind(id)
        .bind(org_id)
        .bind(l.product_id)
        .bind(description)
        .bind(l.quantity.normalize())
        .bind(l.unit_price.map(|d| d.normalize()))
        .bind(l.billing.unwrap_or(LineBilling::Monthly))
        .bind(i32::try_from(i).unwrap_or(i32::MAX))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &AgreementInput,
) -> ContractsResult<Agreement> {
    let c = check(input)?;
    let descriptions = check_refs(&mut *conn, org_id, input, &c.currency).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO contract_agreements (id, org_id, name, kind, status, starts_on, ends_on, auto_renew, \
            renewal_notice_days, currency, priority, covers_all_work_types, monthly_minutes, block_minutes, \
            billing_increment, notes, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(input.kind)
    .bind(c.status)
    .bind(input.starts_on)
    .bind(input.ends_on)
    .bind(input.auto_renew)
    .bind(input.renewal_notice_days)
    .bind(&c.currency)
    .bind(input.priority)
    .bind(c.covers_all)
    .bind(c.monthly_minutes)
    .bind(c.block_minutes)
    .bind(input.billing_increment)
    .bind(&input.notes)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    set_details(&mut *conn, id, org_id, input, &c, &descriptions).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("agreement.created")
            .org(org_id)
            .target("agreement", id)
            .details(json!({ "name": c.name, "kind": input.kind, "status": c.status })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &AgreementInput,
) -> ContractsResult<Agreement> {
    let c = check(input)?;
    let current = get(&mut *conn, id).await?;
    let descriptions = check_refs(&mut *conn, current.org_id, input, &c.currency).await?;
    let changed = sqlx::query(
        "UPDATE contract_agreements SET name = $2, kind = $3, status = $4, starts_on = $5, ends_on = $6, \
                auto_renew = $7, renewal_notice_days = $8, currency = $9, priority = $10, \
                covers_all_work_types = $11, monthly_minutes = $12, block_minutes = $13, \
                billing_increment = $14, notes = $15, version = version + 1, updated_at = now() \
          WHERE id = $1 AND ($16::integer IS NULL OR version = $16)",
    )
    .bind(id)
    .bind(&c.name)
    .bind(input.kind)
    .bind(c.status)
    .bind(input.starts_on)
    .bind(input.ends_on)
    .bind(input.auto_renew)
    .bind(input.renewal_notice_days)
    .bind(&c.currency)
    .bind(input.priority)
    .bind(c.covers_all)
    .bind(c.monthly_minutes)
    .bind(c.block_minutes)
    .bind(input.billing_increment)
    .bind(&input.notes)
    .bind(input.version)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ContractsError::VersionConflict);
    }
    set_details(&mut *conn, id, current.org_id, input, &c, &descriptions).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("agreement.updated")
            .org(current.org_id)
            .target("agreement", id)
            .details(json!({
                "name": c.name, "kind": input.kind, "status": c.status,
                "previousStatus": current.status,
            })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a draft. Agreements that were active are cancelled instead, so
/// what was billed under them stays explained.
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> ContractsResult<()> {
    let current = get(&mut *conn, id).await?;
    if current.status != AgreementStatus::Draft {
        return Err(ContractsError::Conflict(
            "only drafts can be deleted; cancel it instead".into(),
        ));
    }
    sqlx::query("DELETE FROM contract_agreements WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("agreement.deleted")
            .org(current.org_id)
            .target("agreement", id)
            .details(json!({ "name": current.name })),
    )
    .await?;
    Ok(())
}
