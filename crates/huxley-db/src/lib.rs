//! Database access for Huxley.
//!
//! - [`connect`] builds the connection pool.
//! - [`migrate`] applies the embedded migrations from `/migrations`.
//! - [`begin_scoped`] opens a transaction with the caller's org scope set, so
//!   PostgreSQL Row-Level Security policies (design doc §4.4) can enforce
//!   isolation as defense in depth alongside the application's own checks.

use std::time::Duration;

use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub use sqlx;

/// Database configuration (`[database]` in `huxley.toml`, or `HUXLEY_DATABASE__*`).
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// `postgres://` connection URL. Supports `HUXLEY_DATABASE__URL_FILE`.
    pub url: SecretString,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,
    /// Per-statement timeout applied to every connection.
    #[serde(default = "default_statement_timeout_ms")]
    pub statement_timeout_ms: u64,
}

fn default_max_connections() -> u32 {
    20
}
fn default_acquire_timeout_secs() -> u64 {
    5
}
fn default_statement_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("invalid database URL")]
    InvalidUrl(#[source] sqlx::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Embedded migrations from the workspace `migrations/` directory.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Creates the connection pool. Connections are opened lazily.
pub fn connect(config: &DatabaseConfig) -> Result<PgPool, DbError> {
    let options: PgConnectOptions = config
        .url
        .expose_secret()
        .parse()
        .map_err(DbError::InvalidUrl)?;
    let options = options
        .application_name("huxley")
        .options([("statement_timeout", config.statement_timeout_ms.to_string())]);

    Ok(PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
        .connect_lazy_with(options))
}

/// Applies all pending migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Checks that the database is reachable (used by `/readyz`).
pub async fn ping(pool: &PgPool) -> Result<(), DbError> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// Which organizations a transaction may see under RLS.
#[derive(Debug, Clone)]
pub enum OrgScope {
    /// Internal system work (migrations, the scheduler) that bypasses org filtering.
    System,
    /// A request limited to these organization ids (already expanded to include
    /// descendants where the caller's role assignments inherit).
    Orgs(Vec<Uuid>),
}

impl OrgScope {
    fn as_setting(&self) -> String {
        match self {
            Self::System => "*".to_owned(),
            Self::Orgs(ids) => ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// Begins a transaction with `huxley.org_scope` set for its duration.
///
/// RLS policies read the setting via `huxley_org_in_scope(org_id)` (defined in
/// the initial migration). `set_config(..., true)` scopes the value to this
/// transaction, so it cannot leak to the next user of a pooled connection.
pub async fn begin_scoped(
    pool: &PgPool,
    scope: &OrgScope,
) -> Result<Transaction<'static, Postgres>, DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('huxley.org_scope', $1, true)")
        .bind(scope.as_setting())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_settings() {
        assert_eq!(OrgScope::System.as_setting(), "*");
        let a = Uuid::nil();
        let b = Uuid::max();
        assert_eq!(OrgScope::Orgs(vec![a, b]).as_setting(), format!("{a},{b}"));
        assert_eq!(OrgScope::Orgs(vec![]).as_setting(), "");
    }
}
