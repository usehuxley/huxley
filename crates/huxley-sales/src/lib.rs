//! The `sales` module (design doc §8.10, ADR-063): **leads**, **opportunities**
//! moving through configurable **stages**, the stage history, and win/loss
//! **reasons**. Quotes, orders and forecasting build on these.
//!
//! Leads and opportunities are organization-scoped (RLS): a lead at the
//! provider organization that captured it, an opportunity at the client it
//! would sell to. Stages and reasons are instance-wide. The API checks
//! `sales:*` permissions and that the module is on. Money is
//! `numeric(18,4)` with an explicit currency, as decimal strings in JSON.

pub mod activities;
pub mod forecast;
pub mod leads;
pub mod opportunities;
pub mod orders;
pub mod quotes;
pub mod review;
pub mod scoping;
pub mod signing;
pub mod stages;

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

/// The module's key in the registry.
pub const MODULE: &str = "sales";

/// Dates as `YYYY-MM-DD` in JSON.
pub(crate) mod date {
    time::serde::format_description!(ymd, Date, "[year]-[month]-[day]");
    pub(crate) use ymd::option;

    /// A required date.
    pub(crate) mod required {
        pub(crate) use super::ymd::{deserialize, serialize};
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SalesError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Org(#[from] huxley_orgs::OrgError),
    #[error(transparent)]
    Crm(#[from] huxley_crm::CrmError),
}

impl SalesError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type SalesResult<T> = Result<T, SalesError>;

/// Trims optional text; empty becomes `None`. Rejects text over `max`
/// characters.
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> SalesResult<Option<String>> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > max {
        return Err(SalesError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    Ok(Some(v.to_owned()))
}

/// Required text: trimmed, 1 to `max` characters.
pub(crate) fn required(field: &str, value: &str, max: usize) -> SalesResult<String> {
    text(field, Some(value), max)?.ok_or_else(|| SalesError::invalid(field, "required"))
}

/// Out as a decimal string without trailing zeros (`"1149"`, `"22.5"`).
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

/// Money, or nothing: a decimal string or a JSON number; missing or blank is `None`.
pub(crate) fn money_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
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

/// Money: a decimal string (`"12.50"`) or a JSON number; missing is zero.
pub(crate) fn money<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    use serde::de::Error;
    match Option::<Json>::deserialize(d)? {
        None | Some(Json::Null) => Ok(Decimal::ZERO),
        Some(Json::String(s)) if s.trim().is_empty() => Ok(Decimal::ZERO),
        Some(Json::String(s)) => Decimal::from_str(s.trim())
            .map_err(|_| D::Error::custom(format!("{s:?} isn't a number"))),
        Some(Json::Number(n)) => Decimal::from_str(&n.to_string())
            .map_err(|_| D::Error::custom(format!("{n} isn't a number we can keep"))),
        Some(other) => Err(D::Error::custom(format!("{other} isn't a number"))),
    }
}

/// A currency code: three capital letters.
pub(crate) fn currency(value: &str) -> SalesResult<String> {
    let v = value.trim().to_ascii_uppercase();
    if v.len() == 3 && v.chars().all(|c| c.is_ascii_uppercase()) {
        Ok(v)
    } else {
        Err(SalesError::invalid(
            "currency",
            "a three-letter code, e.g. USD",
        ))
    }
}
