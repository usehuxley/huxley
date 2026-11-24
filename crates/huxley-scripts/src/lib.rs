//! The script library (design doc §6.9): PowerShell, Python and JavaScript scripts,
//! each version immutable, with a JSON Schema of its parameters. Runners
//! execute them in sandboxes; nothing here runs code. A library at an
//! organization is shared with the organizations below it (`find`).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

/// Longest source accepted, in bytes.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("script not found")]
    NotFound,
    #[error("version not found")]
    VersionNotFound,
    #[error("another script in this organization already has the key {0:?}")]
    KeyTaken(String),
    /// What's wrong, and where: `(field, message)`.
    #[error("{1}")]
    Invalid(String, String),
    #[error("the script is archived")]
    Archived,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type ScriptResult<T> = Result<T, ScriptError>;

fn invalid(field: &str, message: impl Into<String>) -> ScriptError {
    ScriptError::Invalid(field.to_owned(), message.into())
}

fn db(e: huxley_db::DbError) -> ScriptError {
    match e {
        huxley_db::DbError::Sqlx(e) => ScriptError::Db(e),
        other => ScriptError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "script_language", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// PowerShell 7.
    Pwsh,
    /// Python 3.
    Python,
    /// JavaScript, run with Node.js.
    Javascript,
}

impl Language {
    /// The file extension of its source.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Pwsh => "ps1",
            Self::Python => "py",
            Self::Javascript => "js",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScriptSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub language: Language,
    pub current_version: i32,
    pub archived: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A script with its current version.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    #[serde(flatten)]
    pub summary: ScriptSummary,
    pub source: String,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub parameters: Json,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScriptVersion {
    pub version: i32,
    pub notes: Option<String>,
    pub created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A new script.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewScript {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub language: Language,
    pub source: String,
    /// JSON Schema (an object) of its parameters; default: none.
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub parameters: Option<Json>,
}

/// The next version of a script.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewVersion {
    pub source: String,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>)]
    pub parameters: Option<Json>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The version this edit started from: if the script has moved on
    /// since, nothing is saved (409).
    #[serde(default)]
    pub base_version: Option<i32>,
}

fn valid_key(key: &str) -> bool {
    key.len() <= 64
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The parameters schema to store: an object schema that compiles.
fn parameters(given: Option<&Json>) -> ScriptResult<Json> {
    let schema = given
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    if schema["type"] != "object" || !schema["properties"].is_object() {
        return Err(invalid(
            "parameters",
            "an object schema: { type: object, properties: { … } }",
        ));
    }
    if let Err(e) = jsonschema::validator_for(&schema) {
        return Err(invalid(
            "parameters",
            format!("not a valid JSON Schema: {e}"),
        ));
    }
    Ok(schema)
}

fn source(text: &str) -> ScriptResult<()> {
    if text.trim().is_empty() {
        return Err(invalid("source", "is empty"));
    }
    if text.len() > MAX_SOURCE_BYTES {
        return Err(invalid("source", "at most 256 KB"));
    }
    Ok(())
}

const SUMMARY: &str =
    "id, org_id, key, name, description, language, current_version, archived, updated_at";

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &NewScript,
) -> ScriptResult<Script> {
    if !valid_key(&input.key) {
        return Err(invalid(
            "key",
            "lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    if input.name.trim().is_empty() || input.name.chars().count() > 200 {
        return Err(invalid("name", "1 to 200 characters"));
    }
    source(&input.source)?;
    let params = parameters(input.parameters.as_ref())?;
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO scripts (id, org_id, key, name, description, language, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&input.key)
    .bind(input.name.trim())
    .bind(&input.description)
    .bind(input.language)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ScriptError::KeyTaken(input.key.clone()));
    }
    insert_version(conn, ctx, id, 1, org_id, &input.source, &params, None).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("script.created")
            .org(org_id)
            .target("script", id)
            .details(json!({ "key": input.key, "language": input.language })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_version(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
    org_id: Uuid,
    source: &str,
    parameters: &Json,
    notes: Option<&str>,
) -> ScriptResult<()> {
    sqlx::query(
        "INSERT INTO script_versions (script_id, version, org_id, source, parameters, notes, \
                created_by) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(version)
    .bind(org_id)
    .bind(source)
    .bind(parameters)
    .bind(notes.map(str::trim).filter(|n| !n.is_empty()))
    .bind(ctx.actor.id())
    .execute(conn)
    .await?;
    Ok(())
}

/// Saves the next version and makes it current.
pub async fn save_version(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &NewVersion,
) -> ScriptResult<Script> {
    source(&input.source)?;
    let params = parameters(input.parameters.as_ref())?;
    let row: Option<(Uuid, i32, bool)> = sqlx::query_as(
        "SELECT org_id, current_version, archived FROM scripts WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (org_id, current, archived) = row.ok_or(ScriptError::NotFound)?;
    if archived {
        return Err(ScriptError::Archived);
    }
    if input.base_version.is_some_and(|base| base != current) {
        return Err(invalid(
            "baseVersion",
            format!("the script is now at version {current}: reload it and try again"),
        ));
    }
    let version = current + 1;
    insert_version(
        conn,
        ctx,
        id,
        version,
        org_id,
        &input.source,
        &params,
        input.notes.as_deref(),
    )
    .await?;
    sqlx::query("UPDATE scripts SET current_version = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(version)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("script.version_saved")
            .org(org_id)
            .target("script", id)
            .details(json!({ "version": version })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn set_archived(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    archived: bool,
) -> ScriptResult<Script> {
    let script = get(&mut *conn, id).await?;
    sqlx::query("UPDATE scripts SET archived = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(archived)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if archived {
            "script.archived"
        } else {
            "script.restored"
        })
        .org(script.summary.org_id)
        .target("script", id),
    )
    .await
    .map_err(db)?;
    get(conn, id).await
}

pub async fn summary(conn: &mut PgConnection, id: Uuid) -> ScriptResult<ScriptSummary> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM scripts WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(ScriptError::NotFound)
}

/// A script with its current version's source and parameters.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> ScriptResult<Script> {
    let summary = summary(&mut *conn, id).await?;
    let (source, parameters) = version_source(conn, id, summary.current_version).await?;
    Ok(Script {
        summary,
        source,
        parameters,
    })
}

/// One version's source and parameters.
pub async fn version_source(
    conn: &mut PgConnection,
    id: Uuid,
    version: i32,
) -> ScriptResult<(String, Json)> {
    sqlx::query_as(
        "SELECT source, parameters FROM script_versions WHERE script_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(conn)
    .await?
    .ok_or(ScriptError::VersionNotFound)
}

pub async fn versions(conn: &mut PgConnection, id: Uuid) -> ScriptResult<Vec<ScriptVersion>> {
    summary(&mut *conn, id).await?;
    Ok(sqlx::query_as(
        "SELECT version, notes, created_by, created_at FROM script_versions \
          WHERE script_id = $1 ORDER BY version DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Scripts in the caller's scope, by name.
pub async fn list(conn: &mut PgConnection) -> ScriptResult<Vec<ScriptSummary>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM scripts ORDER BY archived, lower(name), id"
    )))
    .fetch_all(conn)
    .await?)
}

/// The script `key` refers to from `org_id`: its own or the nearest
/// organization above it that has one (not archived).
pub async fn find(
    conn: &mut PgConnection,
    org_id: Uuid,
    key: &str,
) -> ScriptResult<Option<ScriptSummary>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.org_id, s.key, s.name, s.description, s.language, s.current_version, \
                s.archived, s.updated_at \
           FROM scripts s JOIN organizations o ON o.id = s.org_id \
          WHERE s.key = $1 AND NOT s.archived \
            AND o.path @> (SELECT path FROM organizations WHERE id = $2) \
          ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(key)
    .bind(org_id)
    .fetch_optional(conn)
    .await?)
}

/// Checks a script step's inputs against the script's parameter schema.
pub fn check_inputs(parameters: &Json, inputs: &Json) -> Result<(), String> {
    if parameters.as_object().is_none_or(serde_json::Map::is_empty) {
        return Ok(());
    }
    let validator = jsonschema::validator_for(parameters)
        .map_err(|e| format!("the script's parameters aren't a valid schema: {e}"))?;
    let problems: Vec<String> = validator
        .iter_errors(inputs)
        .map(|e| {
            let at = e.instance_path().to_string();
            if at.is_empty() {
                e.to_string()
            } else {
                format!("{at}: {e}")
            }
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_must_be_an_object_schema() {
        assert!(parameters(None).is_ok());
        assert!(
            parameters(Some(
                &json!({ "type": "object", "properties": { "upn": { "type": "string" } } })
            ))
            .is_ok()
        );
        assert!(parameters(Some(&json!({ "type": "string" }))).is_err());
        assert!(source("   ").is_err());
        let schema = json!({ "type": "object", "required": ["upn"],
                             "properties": { "upn": { "type": "string" } } });
        assert!(check_inputs(&schema, &json!({ "upn": "a@b" })).is_ok());
        assert!(check_inputs(&schema, &json!({})).is_err());
        assert!(check_inputs(&json!({}), &json!({ "any": 1 })).is_ok());
        assert!(source(&"x".repeat(MAX_SOURCE_BYTES + 1)).is_err());
    }
}
