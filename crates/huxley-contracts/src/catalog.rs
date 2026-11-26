//! The product catalog: products and services, with SKUs, vendors, cost and
//! price, and bundles made of them.
//!
//! - A SKU is unique within its organization (any case).
//! - A **bundle** lists other products (not bundles) of its organization,
//!   each with a quantity. Its price is its own if set, else what its
//!   components add up to (`componentsPrice`).
//! - Products in a bundle can't be deleted; retire them (`active: false`)
//!   instead, which keeps them on what already uses them.

use std::collections::HashMap;
use std::str::FromStr;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{ContractsError, ContractsResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = CatalogKind)]
pub enum Kind {
    Product,
    Service,
    Bundle,
}

/// How often it's charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = CatalogBilling)]
pub enum Billing {
    OneTime,
    Monthly,
    Quarterly,
    Annual,
}

/// One of a bundle's components.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = CatalogComponent)]
pub struct Component {
    #[serde(skip)]
    pub bundle_id: Uuid,
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub unit: String,
    #[serde(serialize_with = "plain_one")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub price: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = CatalogProduct)]
pub struct Product {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sku: String,
    pub name: String,
    pub description: String,
    pub kind: Kind,
    pub category: Option<String>,
    pub vendor: Option<String>,
    pub manufacturer_part: Option<String>,
    /// What one is: each, hour, month, user, device…
    pub unit: String,
    /// ISO 4217, e.g. `USD`.
    pub currency: String,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub cost: Option<Decimal>,
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub price: Option<Decimal>,
    pub billing: Billing,
    pub taxable: bool,
    /// Retired products stay on what uses them, but aren't offered.
    pub active: bool,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// A bundle's components.
    #[sqlx(skip)]
    pub components: Vec<Component>,
    /// A bundle's components' prices × quantities, if they all have one.
    #[sqlx(skip)]
    #[serde(serialize_with = "plain")]
    #[schema(value_type = Option<String>)]
    pub components_price: Option<Decimal>,
}

/// Out as a decimal string without trailing zeros (`"1149"`, `"22.5"`).
#[allow(clippy::ref_option, clippy::trivially_copy_pass_by_ref)]
pub(crate) fn plain<S: serde::Serializer>(
    value: &Option<Decimal>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(d) => s.serialize_str(&d.normalize().to_string()),
        None => s.serialize_none(),
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn plain_one<S: serde::Serializer>(value: &Decimal, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.normalize().to_string())
}

/// Money and quantities: a decimal string (`"12.50"`) or a JSON number.
pub(crate) fn decimal<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
    use serde::de::Error;
    match Option::<Json>::deserialize(d)? {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Json::String(s)) => Decimal::from_str(s.trim())
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{s:?} isn't a number"))),
        Some(Json::Number(n)) => Decimal::from_str(&n.to_string())
            .or_else(|_| Decimal::from_scientific(&n.to_string()))
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{n} isn't a number we can keep"))),
        Some(other) => Err(D::Error::custom(format!("{other} isn't a number"))),
    }
}

pub(crate) fn quantity<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    use serde::de::Error;
    decimal(d)?.ok_or_else(|| D::Error::custom("a quantity is needed"))
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CatalogComponentInput)]
pub struct ComponentInput {
    pub product_id: Uuid,
    #[serde(deserialize_with = "quantity")]
    #[schema(value_type = String)]
    pub quantity: Decimal,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CatalogProductInput)]
pub struct ProductInput {
    pub sku: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: Kind,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub manufacturer_part: Option<String>,
    /// Default `each`.
    #[serde(default)]
    pub unit: Option<String>,
    /// Default `USD`.
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default, deserialize_with = "decimal")]
    #[schema(value_type = Option<String>)]
    pub cost: Option<Decimal>,
    #[serde(default, deserialize_with = "decimal")]
    #[schema(value_type = Option<String>)]
    pub price: Option<Decimal>,
    /// Default `one_time`.
    #[serde(default)]
    pub billing: Option<Billing>,
    #[serde(default = "yes")]
    pub taxable: bool,
    #[serde(default = "yes")]
    pub active: bool,
    /// A bundle's components (bundles only).
    #[serde(default)]
    pub components: Vec<ComponentInput>,
    /// The version you read (changes only).
    #[serde(default)]
    pub version: Option<i32>,
}

/// What to list.
#[derive(Debug, Clone, Default, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    /// Words in the name, or the start of the SKU.
    pub q: Option<String>,
    pub kind: Option<Kind>,
    pub category: Option<String>,
    /// Only active (or only retired) products.
    pub active: Option<bool>,
    pub org_id: Option<Uuid>,
}

const SELECT: &str = "SELECT id, org_id, sku, name, description, kind, category, vendor, \
    manufacturer_part, unit, currency, cost, price, billing, taxable, active, version, \
    created_at, updated_at FROM catalog_products";

/// The most products a list returns.
pub const MAX_LIST: i64 = 1000;

pub async fn list(conn: &mut PgConnection, filter: &Filter) -> ContractsResult<Vec<Product>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut products: Vec<Product> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR name ILIKE '%' || $1 || '%' OR lower(sku) LIKE lower($1) || '%') \
           AND ($2::text IS NULL OR kind = $2) \
           AND ($3::text IS NULL OR lower(category) = lower($3)) \
           AND ($4::boolean IS NULL OR active = $4) \
           AND ($5::uuid IS NULL OR org_id = $5) \
         ORDER BY lower(name), sku LIMIT {MAX_LIST}"
    )))
    .bind(q)
    .bind(filter.kind)
    .bind(filter.category.as_deref())
    .bind(filter.active)
    .bind(filter.org_id)
    .fetch_all(&mut *conn)
    .await?;
    with_components(conn, &mut products).await?;
    Ok(products)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> ContractsResult<Product> {
    let product: Product = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(ContractsError::NotFound)?;
    let mut one = vec![product];
    with_components(conn, &mut one).await?;
    Ok(one.remove(0))
}

/// Fills in bundles' components and what they add up to.
async fn with_components(conn: &mut PgConnection, products: &mut [Product]) -> ContractsResult<()> {
    let bundles: Vec<Uuid> = products
        .iter()
        .filter(|p| p.kind == Kind::Bundle)
        .map(|p| p.id)
        .collect();
    if bundles.is_empty() {
        return Ok(());
    }
    let rows: Vec<Component> = sqlx::query_as(
        "SELECT i.bundle_id, i.product_id, p.sku, p.name, p.unit, i.quantity, p.price \
           FROM catalog_bundle_items i JOIN catalog_products p ON p.id = i.product_id \
          WHERE i.bundle_id = ANY($1) ORDER BY i.position, p.name",
    )
    .bind(&bundles)
    .fetch_all(conn)
    .await?;
    let mut by_bundle: HashMap<Uuid, Vec<Component>> = HashMap::new();
    for c in rows {
        by_bundle.entry(c.bundle_id).or_default().push(c);
    }
    for p in products.iter_mut().filter(|p| p.kind == Kind::Bundle) {
        p.components = by_bundle.remove(&p.id).unwrap_or_default();
        p.components_price = p
            .components
            .iter()
            .map(|c| c.price.map(|price| price * c.quantity))
            .sum::<Option<Decimal>>()
            .map(|d| d.round_dp(4));
    }
    Ok(())
}

/// The largest amount `numeric(18,4)` holds, plus one.
pub(crate) fn too_big() -> Decimal {
    Decimal::from(100_000_000_000_000_i64)
}

pub(crate) fn money(field: &str, value: Option<Decimal>) -> ContractsResult<Option<Decimal>> {
    match value {
        Some(v) if v.is_sign_negative() && !v.is_zero() => {
            Err(ContractsError::invalid(field, "can't be negative"))
        }
        Some(v) if v.scale() > 4 && v.round_dp(4) != v => {
            Err(ContractsError::invalid(field, "at most 4 decimal places"))
        }
        Some(v) if v >= too_big() => Err(ContractsError::invalid(field, "too large")),
        other => Ok(other.map(|v| v.round_dp(4).normalize())),
    }
}

fn optional_text(field: &str, value: Option<&str>, max: usize) -> ContractsResult<Option<String>> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.chars().count() > max => Err(ContractsError::invalid(
            field,
            format!("at most {max} characters"),
        )),
        other => Ok(other.map(str::to_owned)),
    }
}

/// A checked input, ready to write.
struct Clean {
    sku: String,
    name: String,
    category: Option<String>,
    vendor: Option<String>,
    manufacturer_part: Option<String>,
    unit: String,
    currency: String,
    cost: Option<Decimal>,
    price: Option<Decimal>,
    billing: Billing,
}

fn check(input: &ProductInput) -> ContractsResult<Clean> {
    let sku = input.sku.trim().to_owned();
    let mut chars = sku.chars();
    if sku.is_empty()
        || sku.chars().count() > 64
        || !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
    {
        return Err(ContractsError::invalid(
            "sku",
            "1 to 64 letters, digits, dots, dashes, slashes or underscores, starting with a letter or digit",
        ));
    }
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ContractsError::invalid("name", "1 to 200 characters"));
    }
    if input.description.chars().count() > 4000 {
        return Err(ContractsError::invalid(
            "description",
            "at most 4000 characters",
        ));
    }
    let unit = optional_text("unit", input.unit.as_deref(), 32)?.unwrap_or_else(|| "each".into());
    let currency = input
        .currency
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("USD")
        .to_ascii_uppercase();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(ContractsError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ));
    }
    match (input.kind, input.components.is_empty()) {
        (Kind::Bundle, true) => {
            return Err(ContractsError::invalid(
                "components",
                "a bundle needs at least one product",
            ));
        }
        (Kind::Product | Kind::Service, false) => {
            return Err(ContractsError::invalid(
                "components",
                "only bundles have components",
            ));
        }
        _ => {}
    }
    for (i, c) in input.components.iter().enumerate() {
        if c.quantity <= Decimal::ZERO
            || c.quantity >= too_big()
            || c.quantity.round_dp(4) != c.quantity
        {
            return Err(ContractsError::invalid(
                &format!("components/{i}/quantity"),
                "more than 0, with at most 4 decimal places",
            ));
        }
        if input.components[..i]
            .iter()
            .any(|o| o.product_id == c.product_id)
        {
            return Err(ContractsError::invalid(
                &format!("components/{i}/productId"),
                "already in the bundle",
            ));
        }
    }
    Ok(Clean {
        sku,
        name,
        category: optional_text("category", input.category.as_deref(), 100)?,
        vendor: optional_text("vendor", input.vendor.as_deref(), 200)?,
        manufacturer_part: optional_text(
            "manufacturerPart",
            input.manufacturer_part.as_deref(),
            100,
        )?,
        unit,
        currency,
        cost: money("cost", input.cost)?,
        price: money("price", input.price)?,
        billing: input.billing.unwrap_or(Billing::OneTime),
    })
}

fn taken(sku: &str) -> impl FnOnce(sqlx::Error) -> ContractsError + '_ {
    move |e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("catalog_products_sku") => {
            ContractsError::Conflict(format!("another product here has the SKU {sku}"))
        }
        _ => ContractsError::Sqlx(e),
    }
}

/// Refuses a SKU another product of `org_id` has (checked first, so the
/// caller's transaction stays usable; the unique index backs it up).
async fn sku_free(
    conn: &mut PgConnection,
    org_id: Uuid,
    sku: &str,
    except: Option<Uuid>,
) -> ContractsResult<()> {
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM catalog_products \
          WHERE org_id = $1 AND lower(sku) = lower($2) AND id IS DISTINCT FROM $3)",
    )
    .bind(org_id)
    .bind(sku)
    .bind(except)
    .fetch_one(conn)
    .await?;
    if taken {
        return Err(ContractsError::Conflict(format!(
            "another product here has the SKU {sku}"
        )));
    }
    Ok(())
}

/// Checks a bundle's components: each a product (not a bundle) of the
/// bundle's organization, and not the bundle itself.
async fn check_components(
    conn: &mut PgConnection,
    bundle: Option<Uuid>,
    org_id: Uuid,
    components: &[ComponentInput],
) -> ContractsResult<()> {
    for (i, c) in components.iter().enumerate() {
        let field = format!("components/{i}/productId");
        if Some(c.product_id) == bundle {
            return Err(ContractsError::invalid(
                &field,
                "a bundle can't contain itself",
            ));
        }
        let found: Option<(Uuid, Kind)> =
            sqlx::query_as("SELECT org_id, kind FROM catalog_products WHERE id = $1")
                .bind(c.product_id)
                .fetch_optional(&mut *conn)
                .await?;
        match found {
            None => return Err(ContractsError::invalid(&field, "no such product")),
            Some((org, _)) if org != org_id => {
                return Err(ContractsError::invalid(
                    &field,
                    "a product of the bundle's organization",
                ));
            }
            Some((_, Kind::Bundle)) => {
                return Err(ContractsError::invalid(
                    &field,
                    "bundles can't contain bundles",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Replaces a bundle's components (already checked).
async fn set_components(
    conn: &mut PgConnection,
    bundle: Uuid,
    org_id: Uuid,
    components: &[ComponentInput],
) -> ContractsResult<()> {
    sqlx::query("DELETE FROM catalog_bundle_items WHERE bundle_id = $1")
        .bind(bundle)
        .execute(&mut *conn)
        .await?;
    for (i, c) in components.iter().enumerate() {
        sqlx::query(
            "INSERT INTO catalog_bundle_items (bundle_id, product_id, org_id, quantity, position) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(bundle)
        .bind(c.product_id)
        .bind(org_id)
        .bind(c.quantity.normalize())
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
    input: &ProductInput,
) -> ContractsResult<Product> {
    let c = check(input)?;
    sku_free(&mut *conn, org_id, &c.sku, None).await?;
    check_components(&mut *conn, None, org_id, &input.components).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO catalog_products (id, org_id, sku, name, description, kind, category, vendor, \
            manufacturer_part, unit, currency, cost, price, billing, taxable, active, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.sku)
    .bind(&c.name)
    .bind(&input.description)
    .bind(input.kind)
    .bind(&c.category)
    .bind(&c.vendor)
    .bind(&c.manufacturer_part)
    .bind(&c.unit)
    .bind(&c.currency)
    .bind(c.cost)
    .bind(c.price)
    .bind(c.billing)
    .bind(input.taxable)
    .bind(input.active)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await
    .map_err(taken(&c.sku))?;
    set_components(&mut *conn, id, org_id, &input.components).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("catalog_product.created")
            .org(org_id)
            .target("catalog_product", id)
            .details(json!({ "sku": c.sku, "name": c.name, "kind": input.kind })),
    )
    .await?;
    get(conn, id).await
}

/// Changes a product. Its kind can't change while it's a bundle's
/// component (into a bundle) or a bundle with components (out of one) —
/// components are replaced with the input's.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ProductInput,
) -> ContractsResult<Product> {
    let c = check(input)?;
    let current = get(&mut *conn, id).await?;
    sku_free(&mut *conn, current.org_id, &c.sku, Some(id)).await?;
    check_components(&mut *conn, Some(id), current.org_id, &input.components).await?;
    if input.kind == Kind::Bundle && current.kind != Kind::Bundle {
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM catalog_bundle_items WHERE product_id = $1)",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if used {
            return Err(ContractsError::Conflict(
                "it's in a bundle, so it can't become one".into(),
            ));
        }
    }
    let changed = sqlx::query(
        "UPDATE catalog_products SET sku = $2, name = $3, description = $4, kind = $5, category = $6, \
                vendor = $7, manufacturer_part = $8, unit = $9, currency = $10, cost = $11, price = $12, \
                billing = $13, taxable = $14, active = $15, version = version + 1, updated_at = now() \
          WHERE id = $1 AND ($16::integer IS NULL OR version = $16)",
    )
    .bind(id)
    .bind(&c.sku)
    .bind(&c.name)
    .bind(&input.description)
    .bind(input.kind)
    .bind(&c.category)
    .bind(&c.vendor)
    .bind(&c.manufacturer_part)
    .bind(&c.unit)
    .bind(&c.currency)
    .bind(c.cost)
    .bind(c.price)
    .bind(c.billing)
    .bind(input.taxable)
    .bind(input.active)
    .bind(input.version)
    .execute(&mut *conn)
    .await
    .map_err(taken(&c.sku))?
    .rows_affected();
    if changed == 0 {
        return Err(ContractsError::VersionConflict);
    }
    set_components(&mut *conn, id, current.org_id, &input.components).await?;
    let mut fields = Vec::new();
    for (name, was, now) in [
        (
            "cost",
            current.cost.map(|d| d.to_string()),
            c.cost.map(|d| d.to_string()),
        ),
        (
            "price",
            current.price.map(|d| d.to_string()),
            c.price.map(|d| d.to_string()),
        ),
        (
            "active",
            Some(current.active.to_string()),
            Some(input.active.to_string()),
        ),
    ] {
        if was != now {
            fields.push(json!({ "field": name, "from": was, "to": now }));
        }
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("catalog_product.updated")
            .org(current.org_id)
            .target("catalog_product", id)
            .details(json!({ "sku": c.sku, "changes": fields })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a product that no bundle uses (retire it otherwise).
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> ContractsResult<()> {
    let current = get(&mut *conn, id).await?;
    let used_in: Option<String> = sqlx::query_scalar(
        "SELECT p.name FROM catalog_bundle_items i JOIN catalog_products p ON p.id = i.bundle_id \
          WHERE i.product_id = $1 ORDER BY p.name LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(bundle) = used_in {
        return Err(ContractsError::Conflict(format!(
            "it's in the bundle {bundle:?}; retire it instead, or take it out of the bundle first"
        )));
    }
    sqlx::query("DELETE FROM catalog_products WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("catalog_product.deleted")
            .org(current.org_id)
            .target("catalog_product", id)
            .details(json!({ "sku": current.sku, "name": current.name })),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_reads_strings_and_numbers() {
        let input: ProductInput = serde_json::from_value(json!({
            "sku": "M365-BP", "name": "Microsoft 365 Business Premium", "kind": "service",
            "cost": "18.5", "price": 22.0,
        }))
        .unwrap();
        assert_eq!(input.cost, Some(Decimal::new(185, 1)));
        assert_eq!(input.price, Some(Decimal::new(22, 0)));
        let clean = check(&input).unwrap();
        assert_eq!(
            (clean.unit.as_str(), clean.currency.as_str()),
            ("each", "USD")
        );
        // Out as a decimal string.
        assert_eq!(
            serde_json::to_value(Decimal::new(2250, 2)).unwrap(),
            json!("22.50")
        );
    }

    #[test]
    fn bad_money_and_bundles_are_refused() {
        let base = json!({ "sku": "X", "name": "X", "kind": "product" });
        let with = |extra: Json| {
            let mut v = base.clone();
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<ProductInput>(v)
        };
        assert!(check(&with(json!({ "price": "-1" })).unwrap()).is_err());
        assert!(check(&with(json!({ "price": "1.23456" })).unwrap()).is_err());
        assert!(check(&with(json!({ "price": "1.2340" })).unwrap()).is_ok());
        assert!(with(json!({ "price": "lots" })).is_err());
        assert!(check(&with(json!({ "kind": "bundle" })).unwrap()).is_err());
        let p = Uuid::now_v7();
        let parts = json!([{ "productId": p, "quantity": 1 }]);
        assert!(check(&with(json!({ "components": parts })).unwrap()).is_err());
        let twice = json!([{ "productId": p, "quantity": 1 }, { "productId": p, "quantity": "2" }]);
        assert!(check(&with(json!({ "kind": "bundle", "components": twice })).unwrap()).is_err());
        assert!(check(&with(json!({ "sku": "-bad" })).unwrap()).is_err());
        assert!(check(&with(json!({ "currency": "usd" })).unwrap()).is_ok());
        assert!(check(&with(json!({ "currency": "dollars" })).unwrap()).is_err());
    }
}
