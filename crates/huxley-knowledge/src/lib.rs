//! The `knowledge` module (design doc §8.5): articles with revisions, a
//! review flow (draft → in review → published), categories and full-text
//! search. An article belongs to an organization: the MSP's for shared
//! know-how, a client's for its own documentation. Records are
//! organization-scoped (RLS); the API checks `kb:*` permissions and that the
//! module is on.

pub mod articles;
pub mod categories;
mod markdown;

pub use markdown::render;

/// The module's key in the registry.
pub const MODULE: &str = "knowledge";

#[derive(Debug, thiserror::Error)]
pub enum KbError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    /// The article isn't in a state that allows this (e.g. approving one
    /// that isn't in review).
    #[error("{0}")]
    State(String),
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl KbError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type KbResult<T> = Result<T, KbError>;

/// Trims text; empty becomes `None`. Rejects text over `max` characters and
/// control characters (other than line breaks and tabs).
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> KbResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(KbError::invalid(field, format!("at most {max} characters")));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(KbError::invalid(field, "contains control characters"));
    }
    Ok(Some(value.to_owned()))
}

/// Maps a unique-index violation to a field error.
pub(crate) fn taken(e: sqlx::Error, index: &str, field: &str, message: &str) -> KbError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some(index) => {
            KbError::invalid(field, message)
        }
        _ => KbError::Sqlx(e),
    }
}
