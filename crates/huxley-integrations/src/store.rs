//! Stored integrations: each import is an immutable version holding the
//! spec as given and the definition derived from it (ADR-023).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::diff::{DefinitionDiff, diff};
use crate::{Definition, ImportError, Warning, import_value, parse};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("integration not found")]
    NotFound,
    #[error("another integration in this organization already has the key {0:?}")]
    KeyTaken(String),
    #[error("the key must be lowercase letters, digits and hyphens, starting with a letter")]
    InvalidKey,
    #[error("version {0} doesn't exist")]
    VersionNotFound(i32),
    /// The integration changed since the update was previewed.
    #[error("the integration is now at version {0}; preview the update again")]
    Stale(i32),
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

fn db(e: huxley_db::DbError) -> StoreError {
    match e {
        huxley_db::DbError::Sqlx(e) => StoreError::Db(e),
        other => StoreError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub current_version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// An integration with its current definition.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationDetail {
    #[serde(flatten)]
    pub summary: IntegrationSummary,
    pub definition: Definition,
    /// What the importer skipped or guessed in the current version.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationVersion {
    pub version: i32,
    pub source_format: String,
    /// The API's own version, from the spec.
    pub api_version: String,
    pub actions: i32,
    pub warnings: i32,
    pub imported_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub imported_at: OffsetDateTime,
}

const SUMMARY: &str = "id, org_id, key, name, description, current_version, created_at, updated_at";

/// A key from a title: `Tickets API` → `tickets-api`.
pub fn key_from(title: &str) -> String {
    let mut key = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else if !key.ends_with('-') && !key.is_empty() {
            key.push('-');
        }
    }
    let key = key.trim_matches('-');
    let key = key.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    let key: String = key.chars().take(64).collect();
    let key = key.trim_end_matches('-').to_owned();
    if key.is_empty() { "api".into() } else { key }
}

/// Whether a key is lowercase letters, digits and hyphens, starting with a
/// letter, at most 64 long.
pub fn valid_key(key: &str) -> bool {
    key.len() <= 64
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Imports a spec as a new integration in `org_id`. The key defaults to
/// one made from the spec's title.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: Option<&str>,
    spec_text: &str,
) -> StoreResult<IntegrationDetail> {
    let spec = parse(spec_text)?;
    let imported = import_value(spec.clone())?;
    let key = key.map_or_else(|| key_from(&imported.definition.title), str::to_owned);
    if !valid_key(&key) {
        return Err(StoreError::InvalidKey);
    }
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO integrations (id, org_id, key, name, description, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&key)
    .bind(&imported.definition.title)
    .bind(&imported.definition.description)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(StoreError::KeyTaken(key));
    }
    insert_version(conn, id, 1, org_id, &spec, &imported, ctx).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("integration.created")
            .org(org_id)
            .target("integration", id)
            .details(json!({
                "key": key,
                "actions": imported.definition.actions.len(),
                "warnings": imported.warnings.len(),
            })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// What importing a newer spec would change, without saving anything.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePreview {
    /// The version this was compared with; pass it when applying.
    pub base_version: i32,
    pub diff: DefinitionDiff,
    /// What the importer worked around in the newer spec.
    pub warnings: Vec<Warning>,
    pub actions: usize,
    /// The API's own version, from the newer spec.
    pub api_version: String,
    /// Published workflows with steps the update would break (among those
    /// the caller can see).
    pub affected: Vec<crate::usage::AffectedWorkflow>,
}

/// Compares a newer spec with the current version.
pub async fn preview(
    conn: &mut PgConnection,
    id: Uuid,
    spec_text: &str,
) -> StoreResult<UpdatePreview> {
    let current = get(&mut *conn, id).await?;
    let imported = import_value(parse(spec_text)?)?;
    let changes = diff(&current.definition, &imported.definition);
    let (aliases,): (Json,) = sqlx::query_as(
        "SELECT aliases FROM integration_versions WHERE integration_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(current.summary.current_version)
    .fetch_one(&mut *conn)
    .await?;
    let affected = crate::usage::affected(
        conn,
        current.summary.org_id,
        &current.summary.key,
        &aliases,
        &changes,
    )
    .await?;
    Ok(UpdatePreview {
        base_version: current.summary.current_version,
        diff: changes,
        warnings: imported.warnings,
        actions: imported.definition.actions.len(),
        api_version: imported.definition.version,
        affected,
    })
}

/// Imports a newer spec as the next version and makes it current. Its key
/// stays; the name and description follow the new spec. With
/// `base_version`, it applies only if that's still the current version
/// (what the update was previewed against).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    spec_text: &str,
    base_version: Option<i32>,
) -> StoreResult<IntegrationDetail> {
    let spec = parse(spec_text)?;
    let imported = import_value(spec.clone())?;
    let current: Option<(i32,)> =
        sqlx::query_as("SELECT current_version FROM integrations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let (current,) = current.ok_or(StoreError::NotFound)?;
    if base_version.is_some_and(|base| base != current) {
        return Err(StoreError::Stale(current));
    }
    // The next number after every version so far (one may have been
    // rolled back from).
    let row: Option<(Uuid, i32)> = sqlx::query_as(
        "UPDATE integrations SET current_version = \
                (SELECT max(version) + 1 FROM integration_versions WHERE integration_id = $1), \
                name = $2, description = $3, updated_at = now() \
          WHERE id = $1 RETURNING org_id, current_version",
    )
    .bind(id)
    .bind(&imported.definition.title)
    .bind(&imported.definition.description)
    .fetch_optional(&mut *conn)
    .await?;
    let (org_id, version) = row.ok_or(StoreError::NotFound)?;
    insert_version(conn, id, version, org_id, &spec, &imported, ctx).await?;
    // Steps written against a renamed action keep working.
    let aliases = carried_aliases(&mut *conn, id, current, &imported.definition).await?;
    sqlx::query(
        "UPDATE integration_versions SET aliases = $3 WHERE integration_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&aliases)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("integration.updated")
            .org(org_id)
            .target("integration", id)
            .details(json!({
                "version": version,
                "actions": imported.definition.actions.len(),
                "warnings": imported.warnings.len(),
            })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// The aliases a new version gets: the previous version's, plus actions
/// renamed since it (same method and path, new key), each pointing at a
/// key that exists in the new version.
async fn carried_aliases(
    conn: &mut PgConnection,
    id: Uuid,
    previous: i32,
    new: &Definition,
) -> StoreResult<Json> {
    let row: Option<(Json, Json)> = sqlx::query_as(
        "SELECT definition, aliases FROM integration_versions \
          WHERE integration_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(previous)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((old, old_aliases)) = row else {
        return Ok(json!({}));
    };
    let old: Definition = serde_json::from_value(old)
        .map_err(|e| StoreError::Db(sqlx::Error::Decode(Box::new(e))))?;
    let renames: std::collections::BTreeMap<String, String> = diff(&old, new)
        .renamed
        .into_iter()
        .map(|r| (r.from, r.to))
        .collect();
    let exists = |key: &str| new.actions.iter().any(|a| a.key == key);
    let mut aliases = serde_json::Map::new();
    // Earlier aliases follow a rename of their target.
    for (from, to) in old_aliases.as_object().into_iter().flatten() {
        let to = to.as_str().unwrap_or_default();
        let to = renames.get(to).map_or(to, String::as_str);
        if exists(to) {
            aliases.insert(from.clone(), json!(to));
        }
    }
    for (from, to) in renames {
        aliases.insert(from, json!(to));
    }
    Ok(Json::Object(aliases))
}

async fn insert_version(
    conn: &mut PgConnection,
    id: Uuid,
    version: i32,
    org_id: Uuid,
    spec: &Json,
    imported: &crate::Imported,
    ctx: &RequestContext,
) -> StoreResult<()> {
    let definition =
        serde_json::to_value(&imported.definition).expect("definitions always serialize");
    let warnings = serde_json::to_value(&imported.warnings).expect("warnings always serialize");
    sqlx::query(
        "INSERT INTO integration_versions (integration_id, version, org_id, source_format, spec, \
                definition, warnings, imported_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(version)
    .bind(org_id)
    .bind(&imported.definition.source_format)
    .bind(spec)
    .bind(definition)
    .bind(warnings)
    .bind(ctx.actor.id())
    .execute(conn)
    .await?;
    Ok(())
}

/// Makes an earlier (or later) imported version current again: a rollback.
pub async fn set_current(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
) -> StoreResult<IntegrationDetail> {
    let before = summary(&mut *conn, id).await?;
    let found: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT definition->>'title', definition->>'description' FROM integration_versions \
          WHERE integration_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(&mut *conn)
    .await?;
    let (title, description) = found.ok_or(StoreError::VersionNotFound(version))?;
    sqlx::query(
        "UPDATE integrations SET current_version = $2, name = COALESCE($3, name), \
                description = $4, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(version)
    .bind(title)
    .bind(description)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("integration.version_changed")
            .org(before.org_id)
            .target("integration", id)
            .details(json!({
                "before": { "currentVersion": before.current_version },
                "after": { "currentVersion": version },
            })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

/// Integrations visible in the caller's scope, by name.
pub async fn list(conn: &mut PgConnection) -> StoreResult<Vec<IntegrationSummary>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM integrations ORDER BY lower(name), id"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn summary(conn: &mut PgConnection, id: Uuid) -> StoreResult<IntegrationSummary> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM integrations WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(StoreError::NotFound)
}

/// An integration with its current version's definition and warnings.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> StoreResult<IntegrationDetail> {
    let summary = summary(&mut *conn, id).await?;
    let (definition, warnings): (Json, Json) = sqlx::query_as(
        "SELECT definition, warnings FROM integration_versions \
          WHERE integration_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(summary.current_version)
    .fetch_one(conn)
    .await?;
    let corrupt = |e: serde_json::Error| StoreError::Db(sqlx::Error::Decode(Box::new(e)));
    Ok(IntegrationDetail {
        summary,
        definition: serde_json::from_value(definition).map_err(corrupt)?,
        warnings: serde_json::from_value(warnings).map_err(corrupt)?,
    })
}

/// Every imported version, newest first.
pub async fn versions(conn: &mut PgConnection, id: Uuid) -> StoreResult<Vec<IntegrationVersion>> {
    summary(&mut *conn, id).await?;
    Ok(sqlx::query_as(
        "SELECT version, source_format, definition->>'version' AS api_version, \
                jsonb_array_length(definition->'actions') AS actions, \
                jsonb_array_length(warnings) AS warnings, imported_by, imported_at \
           FROM integration_versions WHERE integration_id = $1 ORDER BY version DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_from_titles() {
        assert_eq!(key_from("Tickets API"), "tickets-api");
        assert_eq!(key_from("  365 Graph (beta) "), "graph-beta");
        assert_eq!(key_from("!!!"), "api");
        assert!(valid_key(&key_from(&"x".repeat(100))));
        assert!(!valid_key("Bad Key"));
    }
}
