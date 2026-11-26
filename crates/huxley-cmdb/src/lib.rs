//! The `cmdb` module (design doc §8.5): assets (configuration items) with
//! types that bring their own fields, a lifecycle, owner, site and
//! warranty; typed relationships between them; and impact analysis (what
//! breaks if an asset goes down). Records are organization-scoped (RLS);
//! the API checks `asset:*` permissions and that the module is on.

pub mod assets;

/// Dates as `YYYY-MM-DD` in JSON (and `Option<Date>` with
/// `#[serde(default, with = "crate::date::option")]`).
pub(crate) mod date {
    time::serde::format_description!(ymd, Date, "[year]-[month]-[day]");
    pub(crate) use ymd::{deserialize, option, serialize};
}
pub mod mappings;
pub mod relationships;
pub mod tickets;
pub mod types;
pub mod warranties;

/// The module's key in the registry.
pub const MODULE: &str = "cmdb";

#[derive(Debug, thiserror::Error)]
pub enum CmdbError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl CmdbError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

impl From<huxley_entity::EntityError> for CmdbError {
    fn from(e: huxley_entity::EntityError) -> Self {
        match e {
            huxley_entity::EntityError::Invalid { field, message } => {
                Self::Invalid { field, message }
            }
            huxley_entity::EntityError::NotFound => Self::NotFound,
            huxley_entity::EntityError::Db(e) => Self::Sqlx(e),
            other => Self::Invalid {
                field: "attributes".into(),
                message: other.to_string(),
            },
        }
    }
}

pub type CmdbResult<T> = Result<T, CmdbError>;

/// Trims text; empty becomes `None`. Rejects text over `max` characters and
/// control characters (other than line breaks and tabs).
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> CmdbResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(CmdbError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(CmdbError::invalid(field, "contains control characters"));
    }
    Ok(Some(value.to_owned()))
}
