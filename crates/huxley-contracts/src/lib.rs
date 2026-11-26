//! The `contracts` module (design doc §8.7). So far, the **product
//! catalog**: products and services an MSP sells, with SKUs, vendors, cost
//! and price, and bundles of them. Agreements build on it.
//!
//! Records are organization-scoped (RLS); the API checks `catalog:*`
//! permissions and that the module is on. Money is `numeric(18,4)` with an
//! explicit currency, as decimal strings in JSON.

pub mod catalog;

/// The module's key in the registry.
pub const MODULE: &str = "contracts";

#[derive(Debug, thiserror::Error)]
pub enum ContractsError {
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
}

impl ContractsError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type ContractsResult<T> = Result<T, ContractsError>;
