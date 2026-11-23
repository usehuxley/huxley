//! Organization variables (design doc §6.6).
//!
//! A variable set on an org applies to it and everything below, unless a
//! nearer org sets its own; an org can lock a key so nothing below can
//! change it (the shallowest lock wins, as for org settings). Workflows read
//! plain variables as `vars.<key>`. Secret variables are encrypted at rest,
//! never returned by the API, and reach expressions only as `secrets.<key>`.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_crypto::{Binding, Crypto, CryptoError};
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum VariableError {
    #[error("organization not found")]
    OrgNotFound,
    #[error("variable not found")]
    NotFound,
    #[error("variable keys are lowercase letters, digits and underscores, starting with a letter")]
    InvalidKey,
    #[error("{0}")]
    InvalidValue(String),
    #[error("{0} locked this variable for the organizations under it")]
    Locked(String),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type VariableResult<T> = Result<T, VariableError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "variable_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    String,
    Number,
    Bool,
    Json,
    Secret,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OrgRef {
    pub id: Uuid,
    pub name: String,
}

/// A variable's effective value at an org, and why.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedVariable {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: VariableType,
    /// The effective value; always `null` for secrets.
    #[schema(value_type = Option<serde_json::Value>)]
    pub value: Option<Json>,
    pub description: Option<String>,
    /// Where the effective value is set.
    pub source: OrgRef,
    /// Set when an ancestor locked the key: it can't be changed here.
    pub locked_by: Option<OrgRef>,
    /// Whether this org sets the key itself (possibly overridden by a lock).
    pub set_here: bool,
    /// Whether this org locks the key for those below.
    pub locked_here: bool,
}

/// What a run's expressions see: plain values and decrypted secrets.
#[derive(Default, Clone)]
pub struct RunVariables {
    pub vars: Map<String, Json>,
    pub secrets: BTreeMap<String, String>,
}

impl std::fmt::Debug for RunVariables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunVariables")
            .field("vars", &self.vars.keys().collect::<Vec<_>>())
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A variable to set.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariableInput {
    #[serde(rename = "type")]
    pub kind: VariableType,
    /// A string for `string` and `secret`, a number, a boolean, or any JSON.
    #[schema(value_type = serde_json::Value)]
    pub value: Json,
    #[serde(default)]
    pub description: Option<String>,
    /// Stop the organizations under this one changing it.
    #[serde(default)]
    pub locked: bool,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: Uuid,
    key: String,
    #[sqlx(rename = "type")]
    kind: VariableType,
    value: Option<Json>,
    secret_sealed: Option<Vec<u8>>,
    description: Option<String>,
    locked: bool,
    org_id: Uuid,
    org_name: String,
}

fn valid_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Every variable defined at or above `org_id`, nearest first.
async fn candidates(conn: &mut PgConnection, org_id: Uuid) -> VariableResult<Vec<Candidate>> {
    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM organizations WHERE id = $1")
        .bind(org_id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Err(VariableError::OrgNotFound);
    }
    Ok(sqlx::query_as(
        "SELECT v.id, v.key, v.type, v.value, v.secret_sealed, v.description, v.locked, \
                v.org_id, o.name AS org_name \
           FROM org_variables v \
           JOIN organizations o ON o.id = v.org_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC, v.key",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

/// For each key: the winner (shallowest lock, else nearest), the lock from
/// above (if any), and this org's own row.
fn winners(
    candidates: &[Candidate],
    org_id: Uuid,
) -> Vec<(&Candidate, Option<&Candidate>, Option<&Candidate>)> {
    let mut keys: Vec<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .filter_map(|key| {
            let chain: Vec<&Candidate> = candidates.iter().filter(|c| c.key == key).collect();
            let lock = chain.iter().rev().find(|c| c.locked).copied();
            let winner = lock.or_else(|| chain.first().copied())?;
            let locked_by = chain
                .iter()
                .rev()
                .find(|c| c.locked && c.org_id != org_id)
                .copied();
            let local = chain.iter().find(|c| c.org_id == org_id).copied();
            Some((winner, locked_by, local))
        })
        .collect()
}

/// The variables in effect at `org_id`, by key.
pub async fn resolve(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> VariableResult<Vec<ResolvedVariable>> {
    let candidates = candidates(conn, org_id).await?;
    Ok(winners(&candidates, org_id)
        .into_iter()
        .map(|(winner, locked_by, local)| ResolvedVariable {
            key: winner.key.clone(),
            kind: winner.kind,
            value: winner.value.clone(),
            description: winner.description.clone(),
            source: OrgRef {
                id: winner.org_id,
                name: winner.org_name.clone(),
            },
            locked_by: locked_by.map(|c| OrgRef {
                id: c.org_id,
                name: c.org_name.clone(),
            }),
            set_here: local.is_some(),
            locked_here: local.is_some_and(|c| c.locked),
        })
        .collect())
}

fn purpose(key: &str) -> String {
    format!("org_variable:{key}")
}

/// What a run in `org_id` sees, with secrets decrypted.
pub async fn for_run(
    conn: &mut PgConnection,
    crypto: &Crypto,
    org_id: Uuid,
) -> VariableResult<RunVariables> {
    let candidates = candidates(conn, org_id).await?;
    let mut resolved = RunVariables::default();
    for (winner, _, _) in winners(&candidates, org_id) {
        match (&winner.kind, &winner.secret_sealed, &winner.value) {
            (VariableType::Secret, Some(sealed), _) => {
                let binding = Binding {
                    purpose: &purpose(&winner.key),
                    record: winner.id,
                };
                let plain = crypto.decrypt(binding, sealed).await?;
                resolved.secrets.insert(
                    winner.key.clone(),
                    String::from_utf8_lossy(&plain).into_owned(),
                );
            }
            (_, _, Some(value)) => {
                resolved.vars.insert(winner.key.clone(), value.clone());
            }
            _ => {}
        }
    }
    Ok(resolved)
}

fn check_value(kind: VariableType, value: &Json) -> VariableResult<()> {
    let ok = match kind {
        VariableType::String | VariableType::Secret => value.is_string(),
        VariableType::Number => value.is_number(),
        VariableType::Bool => value.is_boolean(),
        VariableType::Json => true,
    };
    if !ok {
        return Err(VariableError::InvalidValue(format!(
            "a {} variable needs a {} value",
            serde_json::to_value(kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            match kind {
                VariableType::String | VariableType::Secret => "text",
                VariableType::Number => "number",
                VariableType::Bool => "true or false",
                VariableType::Json => "JSON",
            }
        )));
    }
    if serde_json::to_vec(value).map_or(true, |b| b.len() > 64 * 1024) {
        return Err(VariableError::InvalidValue(
            "variables are limited to 64 KiB".into(),
        ));
    }
    if kind == VariableType::Secret && value.as_str().is_some_and(str::is_empty) {
        return Err(VariableError::InvalidValue(
            "a secret can't be empty".into(),
        ));
    }
    Ok(())
}

async fn ancestor_lock(
    conn: &mut PgConnection,
    org_id: Uuid,
    key: &str,
) -> VariableResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT o.name FROM org_variables v JOIN organizations o ON o.id = v.org_id \
          WHERE v.key = $2 AND v.locked AND v.org_id <> $1 \
            AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) LIMIT 1",
    )
    .bind(org_id)
    .bind(key)
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|(name,)| name))
}

/// Sets `key` at `org_id`. Secrets are encrypted; their value is never
/// stored or audited in the clear.
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    crypto: &Crypto,
    org_id: Uuid,
    key: &str,
    input: &VariableInput,
) -> VariableResult<()> {
    if !valid_key(key) {
        return Err(VariableError::InvalidKey);
    }
    check_value(input.kind, &input.value)?;
    candidates(&mut *conn, org_id).await?;
    if let Some(name) = ancestor_lock(&mut *conn, org_id, key).await? {
        return Err(VariableError::Locked(name));
    }
    let description = input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM org_variables WHERE org_id = $1 AND key = $2")
            .bind(org_id)
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    let id = existing.map_or_else(huxley_core::new_id, |(id,)| id);
    let (value, sealed) = if input.kind == VariableType::Secret {
        let plain = input.value.as_str().unwrap_or_default();
        let binding = Binding {
            purpose: &purpose(key),
            record: id,
        };
        (
            None,
            Some(crypto.encrypt(org_id, binding, plain.as_bytes()).await?),
        )
    } else {
        (Some(input.value.clone()), None)
    };
    sqlx::query(
        "INSERT INTO org_variables (id, org_id, key, type, value, secret_sealed, description, \
                locked, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         ON CONFLICT (org_id, key) DO UPDATE SET type = EXCLUDED.type, value = EXCLUDED.value, \
                secret_sealed = EXCLUDED.secret_sealed, description = EXCLUDED.description, \
                locked = EXCLUDED.locked, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(id)
    .bind(org_id)
    .bind(key)
    .bind(input.kind)
    .bind(&value)
    .bind(&sealed)
    .bind(description)
    .bind(input.locked)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("variable.set")
            .org(org_id)
            .target("org_variable", id)
            .details(json!({ "key": key, "type": input.kind, "locked": input.locked })),
    )
    .await
    .map_err(|e| VariableError::Db(sqlx::Error::Protocol(e.to_string())))?;
    Ok(())
}

/// Removes `key` from `org_id`; the org inherits again.
pub async fn clear(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: &str,
) -> VariableResult<()> {
    let removed: Option<(Uuid,)> =
        sqlx::query_as("DELETE FROM org_variables WHERE org_id = $1 AND key = $2 RETURNING id")
            .bind(org_id)
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    let (id,) = removed.ok_or(VariableError::NotFound)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("variable.cleared")
            .org(org_id)
            .target("org_variable", id)
            .details(json!({ "key": key })),
    )
    .await
    .map_err(|e| VariableError::Db(sqlx::Error::Protocol(e.to_string())))?;
    Ok(())
}
