//! The `servicedesk` module's core (design doc §8.2, ADR-036): ticket
//! types and statuses, the priority matrix, service queues, tickets, their
//! notes and activity. Functions run in the caller's (scoped) transaction,
//! which decides what's visible; the API checks permissions and that the
//! module is active.

/// Business time, now shared from `huxley-core`.
pub use huxley_core::calendar;
pub mod appointments;
pub mod config;
pub mod dispatch;
pub mod embeddings;
pub mod links;
pub mod mail;
pub mod my_work;
pub mod portal;
pub mod presence;
pub mod queues;
pub mod responses;
pub mod sla;
pub mod templates;
pub mod tickets;
pub mod time;
pub mod triage;
pub mod views;

/// The module's key in the registry.
pub const MODULE: &str = "servicedesk";

#[derive(Debug, thiserror::Error)]
pub enum DeskError {
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

impl DeskError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type DeskResult<T> = Result<T, DeskError>;

/// Trims text; empty becomes `None`. Rejects text longer than `max`
/// characters and control characters other than whitespace.
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> DeskResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(DeskError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(DeskError::invalid(field, "contains control characters"));
    }
    Ok(Some(value.to_owned()))
}
