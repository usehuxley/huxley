//! The `inventory` module (design doc §8.7, ADR-067).
//!
//! - **Locations** an organization keeps stock at: warehouses, bins in
//!   them, technicians' vehicles and client sites.
//! - **Items**: catalog products the organization stocks, tracked by
//!   quantity, by lot, or by serial number.
//! - **Stock** per item, location and lot: on hand, and how much of it is
//!   allocated. Serial-numbered **units** are followed one by one.
//! - **Movements**, a ledger never changed: receive, transfer, adjust,
//!   allocate, release, consume, return and RMA.
//! - **Allocations** set parts aside for a ticket or project; they're then
//!   used (consumed) or released.
//! - **Reorder points** per item and location raise `stock.low` once each
//!   time what's available falls to the point.
//!
//! - **Vendors** and **purchase orders**: drafted, approved above a
//!   threshold, sent, and received into stock in part or in full; lines
//!   bought for a ticket or project are allocated to it on arrival.
//!
//! Everything is organization-scoped (RLS) to the organization that holds
//! the stock. Quantities are `numeric(18,4)`, as decimal strings in JSON.

pub mod items;
pub mod locations;
pub mod orders;
pub mod stock;
pub mod vendors;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

/// Dates as `YYYY-MM-DD` in JSON.
pub(crate) mod date {
    time::serde::format_description!(ymd, Date, "[year]-[month]-[day]");
    pub(crate) use ymd::option;
}

/// The module's key in the registry.
pub const MODULE: &str = "inventory";

#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl InventoryError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type InventoryResult<T> = Result<T, InventoryError>;

/// Trims optional text; empty becomes `None`. Rejects text over `max`
/// characters.
pub(crate) fn text(
    field: &str,
    value: Option<&str>,
    max: usize,
) -> InventoryResult<Option<String>> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > max {
        return Err(InventoryError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    Ok(Some(v.to_owned()))
}

/// Required text: trimmed, 1 to `max` characters.
pub(crate) fn required(field: &str, value: &str, max: usize) -> InventoryResult<String> {
    text(field, Some(value), max)?.ok_or_else(|| InventoryError::invalid(field, "required"))
}

/// Out as a decimal string without trailing zeros (`"12"`, `"2.5"`).
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn plain<S: serde::Serializer>(value: &Decimal, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.normalize().to_string())
}

/// Out as a decimal string without trailing zeros, or null.
#[allow(clippy::ref_option)]
pub(crate) fn plain_opt<S: serde::Serializer>(
    value: &Option<Decimal>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(d) => s.serialize_str(&d.normalize().to_string()),
        None => s.serialize_none(),
    }
}

/// A quantity or amount: a decimal string or a JSON number.
pub(crate) fn number<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    use serde::de::Error;
    match Json::deserialize(d)? {
        Json::String(s) => Decimal::from_str(s.trim())
            .map_err(|_| D::Error::custom(format!("{s:?} isn't a number"))),
        Json::Number(n) => Decimal::from_str(&n.to_string())
            .map_err(|_| D::Error::custom(format!("{n} isn't a number we can keep"))),
        other => Err(D::Error::custom(format!("{other} isn't a number"))),
    }
}

/// A quantity or amount, or nothing (missing, null or blank).
pub(crate) fn number_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
    use serde::de::Error;
    match Option::<Json>::deserialize(d)? {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Json::String(s)) => Decimal::from_str(s.trim())
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{s:?} isn't a number"))),
        Some(Json::Number(n)) => Decimal::from_str(&n.to_string())
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{n} isn't a number we can keep"))),
        Some(other) => Err(D::Error::custom(format!("{other} isn't a number"))),
    }
}
