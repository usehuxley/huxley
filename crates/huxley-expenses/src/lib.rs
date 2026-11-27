//! The `expenses` module (design doc §8.16, ADR-079): what people spend,
//! and mileage, against an in-progress ticket, a project (or a phase), a
//! client, or internally.
//!
//! - **Categories** (instance-wide) set whether entries are billable (and
//!   their markup) and reimbursable by default, a limit, and whether a
//!   receipt is needed. Mileage categories take a distance at the rate in
//!   effect on the day.
//! - **Entries** belong to the person who spent; they can change them
//!   while they're drafts. Policy flags (over the limit, no receipt) are
//!   worked out as they're read.
//! - **Receipts** are kept in object storage, one per entry.
//!
//! Money is `numeric(18,4)` with an explicit currency, as decimal strings.

pub mod categories;
pub mod entries;
pub mod rates;
pub mod reports;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

/// Dates as `YYYY-MM-DD` in JSON.
pub(crate) mod date {
    time::serde::format_description!(ymd, Date, "[year]-[month]-[day]");
    pub(crate) use ymd::option;

    pub(crate) mod required {
        pub(crate) use super::ymd::{deserialize, serialize};
    }
}

/// The module's key in the registry.
pub const MODULE: &str = "expenses";

#[derive(Debug, thiserror::Error)]
pub enum ExpenseError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("{0}")]
    Conflict(String),
    #[error("the file is larger than the {0} MB limit")]
    TooLarge(u64),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Storage(#[from] huxley_storage::StorageError),
}

impl ExpenseError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type ExpenseResult<T> = Result<T, ExpenseError>;

/// Trims optional text; empty becomes `None`. Rejects text over `max`.
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> ExpenseResult<Option<String>> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > max {
        return Err(ExpenseError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    Ok(Some(v.to_owned()))
}

/// Required text: trimmed, 1 to `max` characters.
pub(crate) fn required(field: &str, value: &str, max: usize) -> ExpenseResult<String> {
    text(field, Some(value), max)?.ok_or_else(|| ExpenseError::invalid(field, "required"))
}

/// A currency code: three letters, upper-cased.
pub(crate) fn currency(value: Option<&str>) -> ExpenseResult<String> {
    let c = value.map_or_else(|| "USD".to_owned(), |c| c.trim().to_ascii_uppercase());
    if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase()) {
        Ok(c)
    } else {
        Err(ExpenseError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ))
    }
}

/// Out as a decimal string without trailing zeros.
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

/// An amount: a decimal string or a JSON number, or nothing.
pub(crate) fn number_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
    use serde::de::Error;
    match Json::deserialize(d)? {
        Json::Null => Ok(None),
        Json::String(s) if s.trim().is_empty() => Ok(None),
        Json::String(s) => Decimal::from_str(s.trim())
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{s:?} isn't a number"))),
        Json::Number(n) => Decimal::from_str(&n.to_string())
            .map(Some)
            .map_err(|_| D::Error::custom(format!("{n} isn't a number we can keep"))),
        other => Err(D::Error::custom(format!("{other} isn't a number"))),
    }
}
