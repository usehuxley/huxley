//! The `crm` module's base (design doc §8.1, ADR-035): company details,
//! sites and contacts. Every record belongs to an organization; functions
//! run in the caller's (scoped) transaction, which also decides what's
//! visible. The API checks permissions and that the module is active.

pub mod companies;
pub mod contacts;
pub mod sites;

/// The module's key in the registry.
pub const MODULE: &str = "crm";

#[derive(Debug, thiserror::Error)]
pub enum CrmError {
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

impl CrmError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type CrmResult<T> = Result<T, CrmError>;

/// Trims an optional text field; empty becomes `None`. Rejects text longer
/// than `max` characters and control characters other than newlines.
pub(crate) fn text(field: &str, value: Option<&str>, max: usize) -> CrmResult<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(CrmError::invalid(
            field,
            format!("at most {max} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(CrmError::invalid(field, "contains control characters"));
    }
    Ok(Some(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_fields_are_trimmed_and_bounded() {
        assert_eq!(text("f", Some("  hi "), 10).unwrap().as_deref(), Some("hi"));
        assert_eq!(text("f", Some("   "), 10).unwrap(), None);
        assert_eq!(text("f", None, 10).unwrap(), None);
        assert!(text("f", Some("toolongtext"), 5).is_err());
        assert!(text("f", Some("a\u{7}b"), 5).is_err());
        assert!(text("f", Some("a\nb"), 5).is_ok());
    }
}
