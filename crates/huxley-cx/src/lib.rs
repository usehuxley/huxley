//! Customer experience (design doc §8.9): satisfaction surveys when tickets
//! are resolved (ADR-043). NPS and client health scores follow.

pub mod surveys;

/// The module's key in the registry.
pub const MODULE: &str = "cx";

#[derive(Debug, thiserror::Error)]
pub enum CxError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    /// Answered already, or expired.
    #[error("this survey can't be answered any more")]
    Closed,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl CxError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

impl From<huxley_db::DbError> for CxError {
    fn from(e: huxley_db::DbError) -> Self {
        match e {
            huxley_db::DbError::Sqlx(e) => Self::Db(e),
            other => Self::Db(sqlx::Error::Protocol(other.to_string())),
        }
    }
}

pub type CxResult<T> = Result<T, CxError>;
