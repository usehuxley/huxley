//! Custom roles: named permission sets beside the system roles (design doc
//! §11.3). Callers check `role:manage` first; run in system scope.
//!
//! No escalation: a role may only contain permissions its author holds
//! instance-wide (at the root), and only owners may create roles with `*`.
//! So `role:manage` alone can't be used to grant more than you have.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{AuthzError, AuthzResult, Principal, Role, permissions, role_by_id};

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleInput {
    /// Lowercase letters, digits, underscores. Derived from the name if omitted
    /// (on create; it never changes afterwards).
    pub key: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Permission keys, or `["*"]` (owners only).
    pub permissions: Vec<String>,
}

fn slug(name: &str) -> String {
    let mut key = String::new();
    for c in name.trim().to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c);
        } else if !key.is_empty() && !key.ends_with('_') {
            key.push('_');
        }
    }
    let key = key.trim_end_matches('_');
    if key.starts_with(|c: char| c.is_ascii_alphabetic()) {
        key.chars().take(60).collect()
    } else {
        format!("role_{key}").chars().take(60).collect()
    }
}

/// Validates the fields and the author's right to grant every permission.
fn check(actor: &Principal, input: &RoleInput) -> AuthzResult<(String, String, Vec<String>)> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AuthzError::InvalidRole(
            "Enter a name of 1 to 100 characters.".into(),
        ));
    }
    let description = input.description.trim();
    if description.chars().count() > 500 {
        return Err(AuthzError::InvalidRole(
            "Keep the description under 500 characters.".into(),
        ));
    }
    let mut perms: Vec<String> = input
        .permissions
        .iter()
        .map(|p| p.trim().to_owned())
        .collect();
    perms.sort();
    perms.dedup();
    if let Some(unknown) = perms.iter().find(|p| !permissions::is_known(p)) {
        return Err(AuthzError::InvalidRole(format!(
            "Unknown permission `{unknown}`."
        )));
    }
    if perms.iter().any(|p| p == permissions::WILDCARD) {
        if !actor.is_owner() {
            return Err(AuthzError::Escalation(
                "only owners can create a role with every permission".into(),
            ));
        }
        perms = vec![permissions::WILDCARD.to_owned()];
    }
    let missing: Vec<&str> = perms
        .iter()
        .filter(|p| p.as_str() != permissions::WILDCARD && !actor.can_instance(p))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(AuthzError::Escalation(missing.join(", ")));
    }
    Ok((name.to_owned(), description.to_owned(), perms))
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    actor: &Principal,
    input: &RoleInput,
) -> AuthzResult<Role> {
    let (name, description, perms) = check(actor, input)?;
    let key = input
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map_or_else(|| slug(&name), str::to_owned);
    let key_ok = key.starts_with(|c: char| c.is_ascii_lowercase())
        && key.len() <= 60
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !key_ok {
        return Err(AuthzError::InvalidRole(
            "Use a lowercase letter, then letters, digits, or underscores.".into(),
        ));
    }
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO roles (id, key, name, description, is_system, permissions) VALUES ($1, $2, $3, $4, false, $5)")
        .bind(id)
        .bind(&key)
        .bind(&name)
        .bind(&description)
        .bind(&perms)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.created")
            .target("role", id)
            .details(json!({ "key": key, "permissions": perms })),
    )
    .await?;
    role_by_id(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    actor: &Principal,
    id: Uuid,
    input: &RoleInput,
) -> AuthzResult<Role> {
    let before = role_by_id(conn, id).await?;
    if before.is_system {
        return Err(AuthzError::SystemRole);
    }
    let (name, description, perms) = check(actor, input)?;
    sqlx::query("UPDATE roles SET name = $2, description = $3, permissions = $4, version = version + 1 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(&description)
        .bind(&perms)
        .execute(&mut *conn)
        .await?;
    let added: Vec<&String> = perms
        .iter()
        .filter(|p| !before.permissions.contains(p))
        .collect();
    let removed: Vec<&String> = before
        .permissions
        .iter()
        .filter(|p| !perms.contains(p))
        .collect();
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.updated")
            .target("role", id)
            .details(json!({ "key": before.key, "added": added, "removed": removed })),
    )
    .await?;
    role_by_id(conn, id).await
}

/// Deletes a custom role that nobody holds.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> AuthzResult<()> {
    let role = role_by_id(conn, id).await?;
    if role.is_system {
        return Err(AuthzError::SystemRole);
    }
    let (assigned,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM role_assignments WHERE role_id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if assigned > 0 {
        return Err(AuthzError::RoleInUse(assigned));
    }
    sqlx::query("DELETE FROM roles WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.deleted")
            .target("role", id)
            .details(json!({ "key": role.key })),
    )
    .await?;
    Ok(())
}
