//! The SQL console (design doc §9): read-only queries over the `datasets`
//! schema, for instance admins. Queries run on a separate login whose only
//! privilege is the `huxley_datasets_reader` role ([`check_login`] refuses
//! anything stronger), in a read-only transaction, as a single subquery,
//! with a statement timeout and a row limit.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sqlx::{Executor as _, PgPool};
use utoipa::ToSchema;

/// Most rows a query returns.
pub const MAX_ROWS: usize = 1_000;
/// Longest a query may run.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Longest query text.
pub const MAX_QUERY: usize = 20_000;

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SqlRequest {
    /// One `SELECT` (or `WITH … SELECT`) over the `datasets` schema.
    pub query: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SqlResult {
    pub columns: Vec<String>,
    /// Each row, by column name.
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub rows: Vec<Json>,
    /// More rows matched than were returned.
    pub truncated: bool,
}

/// Checks that the console's login can do nothing but read datasets: not
/// a superuser, can't skip row security, and a member of no role but
/// `huxley_datasets_reader` (so `set_config('role', …)` leads nowhere).
pub async fn check_login(pool: &PgPool) -> Result<(), String> {
    let (superuser, bypass, reader, others): (bool, bool, bool, Vec<String>) = sqlx::query_as(
        "SELECT r.rolsuper, r.rolbypassrls, pg_has_role(current_user, 'huxley_datasets_reader', 'MEMBER'), \
                ARRAY(SELECT o.rolname::text FROM pg_roles o \
                       WHERE o.rolname NOT IN (current_user, 'huxley_datasets_reader') \
                         AND pg_has_role(current_user, o.oid, 'MEMBER')) \
           FROM pg_roles r WHERE r.rolname = current_user",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("couldn't check the console's login: {e}"))?;
    if superuser || bypass {
        return Err(
            "the datasets console login must not be a superuser or bypass row security".into(),
        );
    }
    if !reader {
        return Err("the datasets console login must be a member of huxley_datasets_reader".into());
    }
    if !others.is_empty() {
        return Err(format!(
            "the datasets console login must belong to no other roles (it belongs to {})",
            others.join(", ")
        ));
    }
    Ok(())
}

/// Runs `query` read-only on the console's login.
pub async fn run(pool: &PgPool, query: &str) -> Result<SqlResult, String> {
    let query = query.trim().trim_end_matches(';').trim();
    if query.is_empty() {
        return Err("write a SELECT query".into());
    }
    if query.len() > MAX_QUERY {
        return Err(format!("queries are at most {MAX_QUERY} characters"));
    }
    let failed = |e: sqlx::Error| match e {
        sqlx::Error::Database(d) => d.message().to_owned(),
        other => other.to_string(),
    };
    let mut tx = pool.begin().await.map_err(failed)?;
    tx.execute("SET TRANSACTION READ ONLY")
        .await
        .map_err(failed)?;
    tx.execute(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = {}",
        TIMEOUT.as_millis()
    )))
    .await
    .map_err(failed)?;
    tx.execute("SET LOCAL search_path = datasets, pg_catalog")
        .await
        .map_err(failed)?;
    // A subquery holds one SELECT (data-changing WITH clauses aren't
    // allowed inside one), and a prepared statement can't hold two.
    let inner = format!("SELECT * FROM ({query}\n) q");
    let described = (&mut *tx)
        .describe(sqlx::SqlSafeStr::into_sql_str(sqlx::AssertSqlSafe(inner)))
        .await
        .map_err(failed)?;
    let columns: Vec<String> = described
        .columns()
        .iter()
        .map(|c| sqlx::Column::name(c).to_owned())
        .collect();
    let rows: Vec<Json> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT to_jsonb(q) FROM ({query}\n) q LIMIT {}",
        MAX_ROWS + 1
    )))
    .fetch_all(&mut *tx)
    .await
    .map_err(failed)?;
    tx.rollback().await.map_err(failed)?;
    let truncated = rows.len() > MAX_ROWS;
    Ok(SqlResult {
        columns,
        rows: rows.into_iter().take(MAX_ROWS).collect(),
        truncated,
    })
}
