//! The `billing` module (design doc §8.11, ADR-075): invoices generated
//! from what Huxley tracks, in billing runs.
//!
//! - **Sources**: billable time (after agreement coverage, at the
//!   agreement's rate or the provider's default), delivered sales order
//!   lines, agreements' recurring charges (calendar periods, prorated),
//!   project fixed fees (on completion) and reached milestones. Time on
//!   not-to-exceed projects stops at the cap.
//! - **Billed once**: each source is marked as billed by the invoice line
//!   that took it (per period for recurring charges); voiding gives it back.
//! - **Invoices** are drafts until approved; drafts can be edited (lines
//!   changed by hand are flagged), held and voided. Once out, corrections
//!   are credit memos.
//!
//! - **Review** (ADR-076): rules say which drafts need an internal review
//!   before they can be approved; see [`review`].
//!
//! - **Accounting sync** (ADR-077): approved invoices go to the accounting
//!   system through a pack's workflow; see [`sync`].
//!
//! Money is `numeric(18,4)` with an explicit currency, as decimal strings.

pub mod generate;
pub mod invoices;
pub mod review;
pub mod settings;
pub mod status;
pub mod sync;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

/// Dates as `YYYY-MM-DD` in JSON.
pub(crate) mod date {
    time::serde::format_description!(ymd, Date, "[year]-[month]-[day]");
    pub(crate) use ymd::option;

    /// A required date.
    pub(crate) mod required {
        pub(crate) use super::ymd::{deserialize, serialize};
    }
}

/// The module's key in the registry.
pub const MODULE: &str = "billing";

#[derive(Debug, thiserror::Error)]
pub enum BillingError {
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
    #[error(transparent)]
    Contracts(#[from] huxley_contracts::ContractsError),
}

impl BillingError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type BillingResult<T> = Result<T, BillingError>;

/// Trims optional text; empty becomes `None`. Rejects text over `max`
/// characters.
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> BillingResult<Option<String>> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > max {
        return Err(BillingError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    Ok(Some(v.to_owned()))
}

/// Required text: trimmed, 1 to `max` characters.
pub(crate) fn required(field: &str, value: &str, max: usize) -> BillingResult<String> {
    text(field, Some(value), max)?.ok_or_else(|| BillingError::invalid(field, "required"))
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
