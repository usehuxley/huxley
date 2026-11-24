//! Datasets (design doc §9, ADR-029): typed tables of data kept in
//! Huxley. A definition names its columns (from a fixed set of types) and
//! an optional key; the database creates and migrates its table in the
//! `datasets` schema. Rows belong to organizations (the dataset's own or
//! those below it) and row-level security limits who sees them.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

/// Most columns a dataset may have.
pub const MAX_COLUMNS: usize = 200;
/// Most rows one write may carry.
pub const MAX_ROWS_PER_WRITE: usize = 10_000;
/// Most rows one query returns.
pub const MAX_QUERY_ROWS: i64 = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    #[error("no such dataset")]
    NotFound,
    #[error("a dataset with key {0:?} already exists here")]
    KeyTaken(String),
    #[error("{1}")]
    Invalid(String, String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type DatasetResult<T> = Result<T, DatasetError>;

fn invalid(at: impl Into<String>, message: impl Into<String>) -> DatasetError {
    DatasetError::Invalid(at.into(), message.into())
}

fn db(e: huxley_db::DbError) -> DatasetError {
    match e {
        huxley_db::DbError::Sqlx(e) => DatasetError::Db(e),
        other => DatasetError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

/// A column's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    Text,
    Integer,
    Number,
    Boolean,
    Timestamp,
    Date,
    Json,
    Uuid,
}

impl ColumnType {
    /// Its SQL type (the same mapping as `huxley_dataset_sql_type`).
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "bigint",
            Self::Number => "double precision",
            Self::Boolean => "boolean",
            Self::Timestamp => "timestamptz",
            Self::Date => "date",
            Self::Json => "jsonb",
            Self::Uuid => "uuid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Column {
    /// Lowercase letters, digits and underscores, starting with a letter.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ColumnType,
}

/// A dataset's definition, to create or change it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetInput {
    /// Lowercase letters, digits and hyphens. Fixed once created.
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub columns: Vec<Column>,
    /// Columns whose values identify a row within an organization
    /// (writes with the same key update it). Empty: rows are just added.
    #[serde(default)]
    pub primary_key: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dataset {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    org_id: Uuid,
    key: String,
    name: String,
    description: Option<String>,
    columns: Json,
    primary_key: Vec<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl Row {
    fn into_dataset(self) -> Dataset {
        Dataset {
            id: self.id,
            org_id: self.org_id,
            key: self.key,
            name: self.name,
            description: self.description,
            columns: serde_json::from_value(self.columns).unwrap_or_default(),
            primary_key: self.primary_key,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

const COLUMNS: &str =
    "id, org_id, key, name, description, columns, primary_key, created_at, updated_at";

fn valid_key(key: &str) -> bool {
    (1..=63).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_column(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks a definition (the database checks it again).
pub fn check(input: &DatasetInput) -> DatasetResult<()> {
    if !valid_key(&input.key) {
        return Err(invalid(
            "key",
            "lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    if input.name.trim().is_empty() || input.name.chars().count() > 200 {
        return Err(invalid("name", "1 to 200 characters"));
    }
    if input.columns.is_empty() || input.columns.len() > MAX_COLUMNS {
        return Err(invalid("columns", format!("1 to {MAX_COLUMNS} columns")));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (i, column) in input.columns.iter().enumerate() {
        if !valid_column(&column.name) {
            return Err(invalid(
                format!("columns[{i}].name"),
                "lowercase letters, digits and underscores, starting with a letter",
            ));
        }
        if !seen.insert(column.name.as_str()) {
            return Err(invalid(format!("columns[{i}].name"), "appears twice"));
        }
    }
    for (i, key) in input.primary_key.iter().enumerate() {
        if !seen.contains(key.as_str()) {
            return Err(invalid(
                format!("primaryKey[{i}]"),
                format!("{key:?} isn't a column"),
            ));
        }
    }
    Ok(())
}

/// A failed `huxley_dataset_apply` (e.g. values that don't convert) as a
/// validation error; other errors as they are.
fn apply_error(e: sqlx::Error) -> DatasetError {
    match &e {
        sqlx::Error::Database(d)
            if d.code()
                .is_some_and(|c| c.starts_with("22") || c == "P0001" || c == "23505") =>
        {
            invalid(
                "columns",
                format!("the table can't be changed that way: {}", d.message()),
            )
        }
        _ => DatasetError::Db(e),
    }
}

async fn apply(conn: &mut PgConnection, id: Uuid) -> DatasetResult<()> {
    sqlx::query("SELECT huxley_dataset_apply($1)")
        .bind(id)
        .execute(conn)
        .await
        .map_err(apply_error)?;
    Ok(())
}

/// Creates a dataset (and its table) at `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &DatasetInput,
) -> DatasetResult<Dataset> {
    check(input)?;
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO datasets (id, org_id, key, name, description, columns, primary_key, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&input.key)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(serde_json::to_value(&input.columns).unwrap_or_default())
    .bind(&input.primary_key)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(DatasetError::KeyTaken(input.key.clone()));
    }
    apply(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dataset.created")
            .org(org_id)
            .target("dataset", id)
            .details(json!({ "key": input.key, "columns": input.columns.len() })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Changes a dataset's definition, and its table to match. Its key stays.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &DatasetInput,
) -> DatasetResult<Dataset> {
    check(input)?;
    let current = get(&mut *conn, id).await?;
    if input.key != current.key {
        return Err(invalid("key", "a dataset's key can't change"));
    }
    sqlx::query(
        "UPDATE datasets SET name = $2, description = $3, columns = $4, primary_key = $5, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(serde_json::to_value(&input.columns).unwrap_or_default())
    .bind(&input.primary_key)
    .execute(&mut *conn)
    .await?;
    apply(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dataset.updated")
            .org(current.org_id)
            .target("dataset", id)
            .details(json!({ "key": current.key, "columns": input.columns.len() })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Deletes a dataset and every row in it.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DatasetResult<()> {
    let current = get(&mut *conn, id).await?;
    sqlx::query("SELECT huxley_dataset_drop($1)")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM datasets WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dataset.deleted")
            .org(current.org_id)
            .target("dataset", id)
            .details(json!({ "key": current.key })),
    )
    .await
    .map_err(db)?;
    Ok(())
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DatasetResult<Dataset> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM datasets WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(DatasetError::NotFound)?;
    Ok(row.into_dataset())
}

/// Every dataset visible in the transaction's scope.
pub async fn list(conn: &mut PgConnection) -> DatasetResult<Vec<Dataset>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM datasets ORDER BY name"
    )))
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Row::into_dataset).collect())
}

/// The dataset `key` for `org_id`: its own, or the nearest above it.
pub async fn find(conn: &mut PgConnection, org_id: Uuid, key: &str) -> DatasetResult<Dataset> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM datasets d JOIN organizations o ON o.id = d.org_id \
          WHERE d.key = $2 AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
        COLUMNS
            .split(", ")
            .map(|c| format!("d.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
    .bind(org_id)
    .bind(key)
    .fetch_optional(conn)
    .await?
    .ok_or(DatasetError::NotFound)?;
    Ok(row.into_dataset())
}

// ---- rows ----

impl Dataset {
    /// Its table, quoted (the name comes from the id alone).
    fn table(&self) -> String {
        format!("datasets.\"ds_{}\"", self.id.simple())
    }

    fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// `name` quoted, if it's one of the dataset's columns.
    fn quoted(&self, name: &str, at: &str) -> DatasetResult<String> {
        self.column(name)
            .map(|c| format!("\"{}\"", c.name))
            .ok_or_else(|| invalid(at, format!("{name:?} isn't a column of this dataset")))
    }
}

/// Checks that rows may belong to `org_id`: the dataset's organization or
/// one below it.
async fn check_org(conn: &mut PgConnection, dataset: &Dataset, org_id: Uuid) -> DatasetResult<()> {
    let below: Option<bool> = sqlx::query_scalar(
        "SELECT o.path <@ d.path FROM organizations o, organizations d WHERE o.id = $1 AND d.id = $2",
    )
    .bind(org_id)
    .bind(dataset.org_id)
    .fetch_optional(conn)
    .await?;
    if below == Some(true) {
        Ok(())
    } else {
        Err(invalid(
            "orgId",
            "rows belong to the dataset's organization or one below it",
        ))
    }
}

/// `rows` as records for `jsonb_populate_recordset`, each tagged with
/// `org_id`. Unknown columns are refused.
fn records(dataset: &Dataset, org_id: Uuid, rows: &[Json]) -> DatasetResult<Json> {
    if rows.len() > MAX_ROWS_PER_WRITE {
        return Err(invalid(
            "rows",
            format!("at most {MAX_ROWS_PER_WRITE} rows at a time"),
        ));
    }
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let Some(object) = row.as_object() else {
            return Err(invalid(
                format!("rows[{i}]"),
                "each row is an object of column values",
            ));
        };
        if let Some(unknown) = object.keys().find(|k| dataset.column(k).is_none()) {
            return Err(invalid(
                format!("rows[{i}].{unknown}"),
                "not a column of this dataset",
            ));
        }
        for key in &dataset.primary_key {
            if object.get(key).is_none_or(Json::is_null) {
                return Err(invalid(
                    format!("rows[{i}].{key}"),
                    "key columns need a value",
                ));
            }
        }
        let mut record = object.clone();
        record.insert("_org_id".into(), json!(org_id));
        out.push(Json::Object(record));
    }
    Ok(Json::Array(out))
}

/// A bad value for a column (e.g. text in an integer column) as a
/// validation error.
fn value_error(e: sqlx::Error) -> DatasetError {
    match &e {
        sqlx::Error::Database(d) if d.code().is_some_and(|c| c.starts_with("22")) => invalid(
            "rows",
            format!("a value doesn't fit its column: {}", d.message()),
        ),
        sqlx::Error::Database(d) if d.code().as_deref() == Some("21000") => {
            invalid("rows", "two rows have the same key")
        }
        _ => DatasetError::Db(e),
    }
}

/// Adds rows for `org_id`; with a key, rows whose key exists are updated.
/// Returns how many rows were written.
pub async fn upsert(
    conn: &mut PgConnection,
    dataset: &Dataset,
    org_id: Uuid,
    rows: &[Json],
) -> DatasetResult<u64> {
    check_org(&mut *conn, dataset, org_id).await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let records = records(dataset, org_id, rows)?;
    let names: Vec<String> = dataset
        .columns
        .iter()
        .map(|c| format!("\"{}\"", c.name))
        .collect();
    let list = names.join(", ");
    let table = dataset.table();
    let conflict = if dataset.primary_key.is_empty() {
        String::new()
    } else {
        let keys: Vec<String> = dataset
            .primary_key
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect();
        let sets: Vec<String> = names
            .iter()
            .map(|n| format!("{n} = EXCLUDED.{n}"))
            .chain(std::iter::once("_synced_at = now()".to_owned()))
            .collect();
        format!(
            " ON CONFLICT (_org_id, {}) DO UPDATE SET {}",
            keys.join(", "),
            sets.join(", ")
        )
    };
    // Only the columns a row gives are set on insert; on update, every
    // column is (missing ones become NULL): a row is the whole record.
    let sql = format!(
        "INSERT INTO {table} (_org_id, {list}) SELECT _org_id, {list} FROM jsonb_populate_recordset(NULL::{table}, $1){conflict}"
    );
    let done = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(records)
        .execute(conn)
        .await
        .map_err(value_error)?;
    Ok(done.rows_affected())
}

/// Replaces every row of `org_id` with `rows` (a full refresh).
pub async fn replace(
    conn: &mut PgConnection,
    dataset: &Dataset,
    org_id: Uuid,
    rows: &[Json],
) -> DatasetResult<u64> {
    check_org(&mut *conn, dataset, org_id).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {} WHERE _org_id = $1",
        dataset.table()
    )))
    .bind(org_id)
    .execute(&mut *conn)
    .await?;
    upsert(conn, dataset, org_id, rows).await
}

/// Which rows: equality filters on columns, and optionally one
/// organization's rows only.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    /// Column values rows must equal (`null` matches missing values).
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub filters: BTreeMap<String, Json>,
    #[serde(default)]
    pub org_id: Option<Uuid>,
    /// A column to sort by; `-name` sorts descending.
    #[serde(default)]
    pub order_by: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// The `WHERE` clause for `query` and its parameters, numbered from `$1`.
fn conditions(dataset: &Dataset, query: &Query) -> DatasetResult<(String, Vec<Json>)> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    if let Some(org_id) = query.org_id {
        params.push(json!(org_id));
        clauses.push(format!("_org_id = (${} #>> '{{}}')::uuid", params.len()));
    }
    for (name, value) in &query.filters {
        let column = dataset.quoted(name, &format!("filters.{name}"))?;
        let kind = dataset.column(name).map_or(ColumnType::Text, |c| c.kind);
        if value.is_null() {
            clauses.push(format!("{column} IS NULL"));
            continue;
        }
        params.push(value.clone());
        let n = params.len();
        clauses.push(if kind == ColumnType::Json {
            format!("{column} = ${n}")
        } else {
            format!("{column} = (${n} #>> '{{}}')::{}", kind.sql())
        });
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    Ok((sql, params))
}

/// Rows, as objects of column values plus `_org_id` and `_synced_at`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rows {
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub rows: Vec<Json>,
    /// How many rows match, in all.
    pub total: i64,
}

/// Rows matching `query` that the transaction's scope can see.
pub async fn query(
    conn: &mut PgConnection,
    dataset: &Dataset,
    query: &Query,
) -> DatasetResult<Rows> {
    let (filter, params) = conditions(dataset, query)?;
    let order = match query.order_by.as_deref() {
        None | Some("") => "_synced_at DESC".to_owned(),
        Some(spec) => {
            let (name, desc) = spec.strip_prefix('-').map_or((spec, false), |n| (n, true));
            format!(
                "{} {}",
                dataset.quoted(name, "orderBy")?,
                if desc { "DESC" } else { "ASC" }
            )
        }
    };
    let limit = query.limit.unwrap_or(100).clamp(1, MAX_QUERY_ROWS);
    let offset = query.offset.unwrap_or(0).max(0);
    let table = dataset.table();
    let select = format!(
        "SELECT to_jsonb(t) FROM {table} t{filter} ORDER BY {order} LIMIT {limit} OFFSET {offset}"
    );
    let mut q = sqlx::query_scalar::<_, Json>(sqlx::AssertSqlSafe(select));
    for p in &params {
        q = q.bind(p);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(value_error)?;
    let count = format!("SELECT count(*) FROM {table} t{filter}");
    let mut c = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count));
    for p in &params {
        c = c.bind(p);
    }
    let total = c.fetch_one(conn).await.map_err(value_error)?;
    Ok(Rows { rows, total })
}

/// Deletes the rows matching `query` (its order, limit and offset are
/// ignored). Returns how many.
pub async fn delete_rows(
    conn: &mut PgConnection,
    dataset: &Dataset,
    query: &Query,
) -> DatasetResult<u64> {
    let (filter, params) = conditions(dataset, query)?;
    let mut q = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {} t{filter}",
        dataset.table()
    )));
    for p in &params {
        q = q.bind(p);
    }
    Ok(q.execute(conn).await.map_err(value_error)?.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DatasetInput {
        DatasetInput {
            key: "m365-users".into(),
            name: "M365 users".into(),
            description: None,
            columns: vec![
                Column {
                    name: "upn".into(),
                    kind: ColumnType::Text,
                },
                Column {
                    name: "licensed".into(),
                    kind: ColumnType::Boolean,
                },
            ],
            primary_key: vec!["upn".into()],
        }
    }

    #[test]
    fn definitions_are_checked() {
        assert!(check(&input()).is_ok());
        let bad = |f: fn(&mut DatasetInput)| {
            let mut i = input();
            f(&mut i);
            check(&i).is_err()
        };
        assert!(bad(|i| i.key = "Bad Key".into()));
        assert!(bad(|i| i.columns[0].name = "_org_id".into()));
        assert!(bad(
            |i| i.columns[0].name = "x\"; DROP TABLE users; --".into()
        ));
        assert!(bad(|i| i.columns[1].name = "upn".into()));
        assert!(bad(|i| i.primary_key = vec!["nope".into()]));
        assert!(bad(|i| i.columns.clear()));
    }
}
