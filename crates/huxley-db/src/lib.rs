//! Database access for Huxley.
//!
//! - [`connect`] builds the runtime pool. Every connection switches to the
//!   `huxley_app` role so PostgreSQL Row-Level Security applies.
//! - [`connect_owner`] builds a pool as the connecting user, for migrations
//!   and admin tooling only.
//! - [`begin_scoped`] opens a transaction with the caller's org scope set, so
//!   RLS policies (design doc §4.4) enforce isolation as defense in depth
//!   alongside the application's own authorization checks.
//! - [`audit`] and [`outbox`] write audit records and domain events inside the
//!   caller's transaction.

pub mod audit;
pub mod outbox;
pub mod scans;
#[cfg(feature = "testing")]
pub mod testing;

use std::time::Duration;

use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Executor as _, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

pub use sqlx;

/// The role runtime connections assume (created by migrations).
pub const APP_ROLE: &str = "huxley_app";
const SET_APP_ROLE: &str = "SET ROLE huxley_app";

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
    /// A read replica for reporting queries (`HUXLEY_DATABASE__READ_URL`).
    /// Without it they use the primary.
    #[serde(default)]
    pub read_url: Option<SecretString>,
    /// Straight to PostgreSQL (`HUXLEY_DATABASE__DIRECT_URL`) when `url` goes
    /// through a transaction pooler: LISTEN connections, migrations and admin
    /// commands use it. Without it they use `url`.
    #[serde(default)]
    pub direct_url: Option<SecretString>,
    /// `url` goes through a transaction-pooling `PgBouncer` (ADR-089). The
    /// pooler must run `SET ROLE huxley_app` and the statement timeout on
    /// each server connection (startup parameters don't pass through); the
    /// server checks it does.
    #[serde(default)]
    pub pooled: bool,
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

fn options(config: &DatabaseConfig) -> Result<PgConnectOptions, DbError> {
    let options: PgConnectOptions = config
        .url
        .expose_secret()
        .parse()
        .map_err(DbError::InvalidUrl)?;
    if config.pooled {
        // PgBouncer refuses unknown startup parameters; it sets the timeout.
        return Ok(options);
    }
    Ok(options
        .application_name("huxley")
        .options([("statement_timeout", config.statement_timeout_ms.to_string())]))
}

/// The configuration for connections straight to PostgreSQL (`direct_url`,
/// or `url` when there's no pooler in between).
fn direct(config: &DatabaseConfig) -> DatabaseConfig {
    match &config.direct_url {
        Some(url) => DatabaseConfig {
            url: url.clone(),
            direct_url: None,
            pooled: false,
            ..config.clone()
        },
        None => DatabaseConfig {
            pooled: false,
            ..config.clone()
        },
    }
}

/// A small pool straight to PostgreSQL for LISTEN connections, which a
/// transaction pooler can't carry.
pub fn connect_listen(config: &DatabaseConfig) -> Result<PgPool, DbError> {
    connect(&DatabaseConfig {
        max_connections: 4,
        ..direct(config)
    })
}

/// When `pooled`, checks that connections through the pooler run as the
/// application role: if the pooler didn't switch role, they'd run as the
/// login and skip row security.
pub async fn check_pooled(pool: &PgPool, config: &DatabaseConfig) -> Result<(), String> {
    if !config.pooled {
        return Ok(());
    }
    let (user, timeout): (String, String) = sqlx::query_as(
        "SELECT session_user::text || '/' || current_user::text, current_setting('statement_timeout')",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("can't reach the database through the pooler: {e}"))?;
    if !user.ends_with("/huxley_app") {
        return Err(format!(
            "database.pooled is set but connections run as {user} (session/current): the pooler must run \
             `SET ROLE huxley_app` on each server connection (PgBouncer: connect_query)"
        ));
    }
    if timeout == "0" {
        tracing::warn!(
            "connections through the pooler have no statement timeout; set it in the pooler's connect_query"
        );
    }
    Ok(())
}

/// Creates the runtime pool. Connections are opened lazily and run as
/// [`APP_ROLE`], so they fail until migrations have created that role.
pub fn connect(config: &DatabaseConfig) -> Result<PgPool, DbError> {
    let options = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs));
    if config.pooled {
        // Behind a transaction pooler the role is the pooler's job, on every
        // server connection. Setting it here would leave it on whichever
        // server connection this landed on, and hide a pooler that doesn't
        // (see `check_pooled`).
        return Ok(options.connect_lazy_with(self::options(config)?));
    }
    Ok(options
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                conn.execute(SET_APP_ROLE).await?;
                Ok(())
            })
        })
        .connect_lazy_with(self::options(config)?))
}

/// The read replica's pool, when `read_url` is set: like [`connect`], for
/// read-only reporting queries.
pub fn connect_read(config: &DatabaseConfig) -> Result<Option<PgPool>, DbError> {
    let Some(url) = &config.read_url else {
        return Ok(None);
    };
    let replica = DatabaseConfig {
        url: url.clone(),
        read_url: None,
        direct_url: None,
        pooled: false,
        ..config.clone()
    };
    connect(&replica).map(Some)
}

/// Creates a pool that connects as the configured user without switching role,
/// straight to PostgreSQL (`direct_url`). It bypasses RLS: use it only for
/// migrations and admin commands.
pub fn connect_owner(config: &DatabaseConfig) -> Result<PgPool, DbError> {
    let config = direct(config);
    Ok(PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
        .connect_lazy_with(options(&config)?))
}

/// Applies all pending migrations. Requires an owner pool.
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
    /// Internal system work (the scheduler, bootstrap, tree maintenance) that
    /// is not limited to particular organizations.
    System,
    /// A request limited to these organization ids (already expanded to
    /// include descendants where the caller's role assignments inherit).
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
/// RLS policies read the setting via `huxley_org_in_scope(org_id)`.
/// `set_config(..., true)` scopes the value to this transaction, so it cannot
/// leak to the next user of a pooled connection.
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

/// Adds one organization to the current transaction's scope.
///
/// Used right after creating an organization the caller was authorized to
/// create, so the rest of the transaction can read it. A no-op in system scope.
pub async fn extend_scope(conn: &mut PgConnection, org_id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "SELECT set_config('huxley.org_scope', \
            CASE coalesce(current_setting('huxley.org_scope', true), '') \
                WHEN '*' THEN '*' \
                WHEN '' THEN $1 \
                ELSE current_setting('huxley.org_scope', true) || ',' || $1 \
            END, true)",
    )
    .bind(org_id.to_string())
    .execute(conn)
    .await?;
    Ok(())
}

/// Name of the constraint a database error violated, if any. Used to map
/// constraint violations to domain errors.
pub fn violated_constraint(error: &sqlx::Error) -> Option<&str> {
    error.as_database_error().and_then(|e| e.constraint())
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
