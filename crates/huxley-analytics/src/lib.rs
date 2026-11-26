//! The semantic data layer (design doc §8.12, ADR-041): a catalog of data
//! sources and a structured query spec, compiled to parameterized SQL that
//! filters to what the caller may see, run read-only with a timeout, and
//! cached briefly.

pub mod catalog;
pub mod dashboards;
pub mod datasets;
pub mod query;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use huxley_db::{OrgScope, begin_scoped};
use serde::Serialize;
use serde_json::Value as Json;
use sqlx::PgPool;
use utoipa::ToSchema;

pub use catalog::{Kind, SOURCES, Source};
pub use query::{Bind, Column, QuerySpec, Scope, compile};

/// How long a query may run.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a result is reused.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_CAPACITY: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    /// The spec is wrong: `(where, what)`.
    #[error("{0}: {1}")]
    Invalid(String, String),
    /// It took longer than [`TIMEOUT`].
    #[error("the query took too long; narrow it down")]
    TooSlow,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type AnalyticsResult<T> = Result<T, AnalyticsError>;

/// A query's answer.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AnalyticsResult)]
pub struct Answer {
    pub columns: Vec<Column>,
    /// One array per row, in column order.
    #[schema(value_type = Vec<Vec<serde_json::Value>>)]
    pub rows: Vec<Vec<Json>>,
    /// More rows matched than the limit.
    pub truncated: bool,
    /// Served from the cache (at most a minute old).
    pub cached: bool,
}

/// Results reused for [`CACHE_TTL`], by scope and spec.
#[derive(Debug, Default)]
pub struct Cache {
    entries: Mutex<HashMap<String, (Instant, Answer)>>,
}

impl Cache {
    fn get(&self, key: &str) -> Option<Answer> {
        let entries = self.entries.lock().ok()?;
        entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, answer)| Answer {
                cached: true,
                ..answer.clone()
            })
    }

    fn put(&self, key: String, answer: &Answer) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= CACHE_CAPACITY {
            entries.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
            if entries.len() >= CACHE_CAPACITY {
                entries.clear();
            }
        }
        entries.insert(key, (Instant::now(), answer.clone()));
    }
}

fn cache_key(spec: &QuerySpec, scope: &Scope, source: &Source) -> String {
    let scope = match scope {
        Scope::All => "*".to_owned(),
        Scope::Orgs(orgs) => {
            let mut ids: Vec<String> = orgs.iter().map(uuid::Uuid::to_string).collect();
            ids.sort();
            ids.join(",")
        }
    };
    // A dataset's columns can change: they're part of the key.
    let shape: Vec<&str> = source.fields.iter().map(|f| &*f.sql).collect();
    format!(
        "{scope}|{}|{}|{}",
        serde_json::to_string(spec).unwrap_or_default(),
        source.from,
        shape.join(",")
    )
}

/// Runs a query for a caller who may see `scope`, reusing a recent result.
pub async fn run(
    pool: &PgPool,
    cache: &Cache,
    spec: &QuerySpec,
    scope: &Scope,
) -> AnalyticsResult<Answer> {
    let source = catalog::source(&spec.source).ok_or_else(|| {
        AnalyticsError::Invalid("source".into(), format!("{:?} isn't a source", spec.source))
    })?;
    run_for(pool, cache, source, spec, scope).await
}

/// Runs a query over `source` (a catalog source, or a dataset's).
pub async fn run_for(
    pool: &PgPool,
    cache: &Cache,
    source: &Source,
    spec: &QuerySpec,
    scope: &Scope,
) -> AnalyticsResult<Answer> {
    let compiled = query::compile_for(source, spec, scope)?;
    let key = cache_key(spec, scope, source);
    if let Some(answer) = cache.get(&key) {
        return Ok(answer);
    }
    let mut tx = begin_scoped(pool, &OrgScope::System)
        .await
        .map_err(|e| match e {
            huxley_db::DbError::Sqlx(e) => AnalyticsError::Db(e),
            other => AnalyticsError::Db(sqlx::Error::Protocol(other.to_string())),
        })?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = {}",
        TIMEOUT.as_millis()
    )))
    .execute(&mut *tx)
    .await?;
    let mut q = sqlx::query_scalar::<_, Json>(sqlx::AssertSqlSafe(compiled.sql.clone()));
    for b in &compiled.binds {
        q = match b {
            Bind::Text(s) => q.bind(s.clone()),
            Bind::Texts(v) => q.bind(v.clone()),
            Bind::Uuid(u) => q.bind(*u),
            Bind::Uuids(v) => q.bind(v.clone()),
        };
    }
    let rows = q.fetch_all(&mut *tx).await.map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("57014") => AnalyticsError::TooSlow,
        sqlx::Error::Database(d) if d.code().as_deref() == Some("22023") => {
            AnalyticsError::Invalid(
                "timezone".into(),
                "an IANA time zone such as Europe/Berlin".into(),
            )
        }
        _ => AnalyticsError::Db(e),
    })?;
    tx.commit().await?;
    let truncated = rows.len() > compiled.limit as usize;
    let rows = rows
        .into_iter()
        .take(compiled.limit as usize)
        .map(|row| {
            (0..compiled.columns.len())
                .map(|i| row[format!("c{i}")].clone())
                .collect()
        })
        .collect();
    let answer = Answer {
        columns: compiled.columns,
        rows,
        truncated,
        cached: false,
    };
    cache.put(key, &answer);
    Ok(answer)
}
