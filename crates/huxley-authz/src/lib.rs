//! Authorization (design doc §11.3).
//!
//! A user's [`Principal`] is the set of their role grants: role permissions ×
//! an org scope, optionally inherited by descendants. Checks answer "may this
//! principal do X at org O?"; [`visible_orgs`] answers "which orgs may they
//! see?", which becomes the request's RLS scope.
//!
//! Evaluation is plain role × scope matching for now. When attribute-based
//! conditions arrive, evaluation moves to Cedar (ADR-006) behind the same
//! `Principal::can` interface.

pub mod mfa_policy;
pub mod permissions;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::{DbError, violated_constraint};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

pub use permissions::{Permission, PermissionScope};

/// Key of the Owner role. Only owners may grant or remove it.
pub const OWNER_ROLE: &str = "owner";

/// One role grant: a role's permissions at an org, optionally inherited.
#[derive(Debug, Clone)]
pub struct Grant {
    pub role_key: String,
    pub org_id: Uuid,
    /// ltree path of `org_id`, e.g. `a1b2….c3d4…`.
    pub org_path: String,
    pub inherit: bool,
    pub permissions: Vec<String>,
}

impl Grant {
    fn covers_path(&self, path: &str) -> bool {
        path == self.org_path
            || (self.inherit
                && path.len() > self.org_path.len()
                && path.starts_with(&self.org_path)
                && path.as_bytes()[self.org_path.len()] == b'.')
    }

    fn has(&self, permission: &str) -> bool {
        self.permissions
            .iter()
            .any(|p| p == permissions::WILDCARD || p == permission)
    }
}

/// A user's effective grants, loaded once per request.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: Uuid,
    pub grants: Vec<Grant>,
    /// Path of the root organization, for instance-wide checks.
    pub root_path: Option<String>,
    /// Policy requires this user to have a second factor ([`mfa_policy`]).
    pub requires_mfa: bool,
}

impl Principal {
    /// May the principal use `permission` at the org with this ltree path?
    pub fn can_at_path(&self, permission: &str, org_path: &str) -> bool {
        self.grants
            .iter()
            .any(|g| g.has(permission) && g.covers_path(org_path))
    }

    /// May the principal use an instance-wide permission?
    pub fn can_instance(&self, permission: &str) -> bool {
        self.root_path
            .as_deref()
            .is_some_and(|root| self.can_at_path(permission, root))
    }

    /// Whether the principal holds the Owner role at the root.
    pub fn is_owner(&self) -> bool {
        self.root_path.as_deref().is_some_and(|root| {
            self.grants
                .iter()
                .any(|g| g.role_key == OWNER_ROLE && g.org_path == root)
        })
    }

    /// Every permission granted anywhere in the tree, with `*` expanded.
    /// For UI hints only (e.g. which navigation to show); never for enforcement.
    pub fn granted_anywhere(&self) -> Vec<&'static str> {
        permissions::ALL
            .iter()
            .map(|p| p.key)
            .filter(|key| self.grants.iter().any(|g| g.has(key)))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("permission denied")]
    Forbidden,
    #[error("role not found")]
    RoleNotFound,
    #[error("assignment not found")]
    AssignmentNotFound,
    #[error("organization not found")]
    OrgNotFound,
    #[error("the user already has this role at this organization")]
    AlreadyAssigned,
    #[error("only an owner can grant or remove the Owner role")]
    OwnerOnly,
    #[error("the instance must keep at least one owner")]
    LastOwner,
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for AuthzError {
    fn from(error: sqlx::Error) -> Self {
        match violated_constraint(&error) {
            Some("role_assignments_unique") => Self::AlreadyAssigned,
            _ => Self::Db(DbError::Sqlx(error)),
        }
    }
}

pub type AuthzResult<T> = Result<T, AuthzError>;

#[derive(sqlx::FromRow)]
struct GrantRow {
    role_key: String,
    org_id: Uuid,
    org_path: String,
    inherit: bool,
    permissions: Vec<String>,
}

/// Loads a user's grants. Run in system scope: grants reference orgs the RLS
/// scope is derived from, so they must be readable before that scope exists.
pub async fn load_principal(conn: &mut PgConnection, user_id: Uuid) -> AuthzResult<Principal> {
    let rows: Vec<GrantRow> = sqlx::query_as(
        "SELECT r.key AS role_key, a.org_id, o.path::text AS org_path, a.inherit, r.permissions \
           FROM role_assignments a \
           JOIN roles r ON r.id = a.role_id \
           JOIN organizations o ON o.id = a.org_id AND o.status = 'active' \
          WHERE a.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await?;
    let root_path: Option<(String,)> =
        sqlx::query_as("SELECT path::text FROM organizations WHERE parent_id IS NULL")
            .fetch_optional(&mut *conn)
            .await?;
    let requires_mfa = mfa_policy::user_requires_mfa(conn, user_id).await?;
    Ok(Principal {
        user_id,
        grants: rows
            .into_iter()
            .map(|r| Grant {
                role_key: r.role_key,
                org_id: r.org_id,
                org_path: r.org_path,
                inherit: r.inherit,
                permissions: r.permissions,
            })
            .collect(),
        root_path: root_path.map(|(p,)| p),
        requires_mfa,
    })
}

/// Ids of the orgs where the principal holds any grant: the RLS scope for
/// their requests. Run in system scope.
pub async fn visible_orgs(
    conn: &mut PgConnection,
    principal: &Principal,
) -> AuthzResult<Vec<Uuid>> {
    let inherited: Vec<String> = principal
        .grants
        .iter()
        .filter(|g| g.inherit)
        .map(|g| g.org_path.clone())
        .collect();
    let exact: Vec<Uuid> = principal
        .grants
        .iter()
        .filter(|g| !g.inherit)
        .map(|g| g.org_id)
        .collect();
    if inherited.is_empty() && exact.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM organizations WHERE path <@ ANY($1::ltree[]) OR id = ANY($2)",
    )
    .bind(&inherited)
    .bind(&exact)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The ltree path of an org (system scope), for permission checks.
pub async fn org_path(conn: &mut PgConnection, org_id: Uuid) -> AuthzResult<String> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT path::text FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(conn)
            .await?;
    row.map(|(p,)| p).ok_or(AuthzError::OrgNotFound)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub is_system: bool,
    pub permissions: Vec<String>,
    /// Holders must have a second factor.
    pub require_mfa: bool,
}

pub async fn list_roles(conn: &mut PgConnection) -> AuthzResult<Vec<Role>> {
    Ok(sqlx::query_as(
        "SELECT id, key, name, description, is_system, permissions, require_mfa FROM roles \
         ORDER BY is_system DESC, name",
    )
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub id: Uuid,
    pub user_id: Uuid,
    pub role_id: Uuid,
    pub role_key: String,
    pub role_name: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub inherit: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT_ASSIGNMENTS: &str = "SELECT a.id, a.user_id, a.role_id, r.key AS role_key, r.name AS role_name, \
    a.org_id, o.name AS org_name, a.inherit, a.created_at \
    FROM role_assignments a JOIN roles r ON r.id = a.role_id JOIN organizations o ON o.id = a.org_id";

/// A user's assignments that are visible in the current RLS scope.
pub async fn user_assignments(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> AuthzResult<Vec<Assignment>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_ASSIGNMENTS} WHERE a.user_id = $1 ORDER BY o.path, r.name"
    )))
    .bind(user_id)
    .fetch_all(conn)
    .await?)
}

pub(crate) async fn role_by_id(conn: &mut PgConnection, role_id: Uuid) -> AuthzResult<Role> {
    sqlx::query_as(
        "SELECT id, key, name, description, is_system, permissions, require_mfa \
           FROM roles WHERE id = $1",
    )
    .bind(role_id)
    .fetch_optional(conn)
    .await?
    .ok_or(AuthzError::RoleNotFound)
}

/// Grants a role. The caller must hold `role_assignment:manage` at the org;
/// granting Owner additionally requires being an owner. Run in system scope
/// after the permission check (the target org may be outside the caller's
/// RLS scope only if they lack access, which the check already rejected).
pub async fn assign(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    actor: &Principal,
    user_id: Uuid,
    role_id: Uuid,
    org_id: Uuid,
    inherit: bool,
) -> AuthzResult<Assignment> {
    let path = org_path(conn, org_id).await?;
    if !actor.can_at_path(permissions::ROLE_ASSIGN, &path) {
        return Err(AuthzError::Forbidden);
    }
    let role = role_by_id(conn, role_id).await?;
    if role.key == OWNER_ROLE && !actor.is_owner() {
        return Err(AuthzError::OwnerOnly);
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user_id)
    .bind(role_id)
    .bind(org_id)
    .bind(inherit)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.assigned")
            .org(org_id)
            .target("user", user_id)
            .details(json!({ "role": role.key, "inherit": inherit, "assignmentId": id })),
    )
    .await?;
    fetch_assignment(conn, id).await
}

async fn fetch_assignment(conn: &mut PgConnection, id: Uuid) -> AuthzResult<Assignment> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_ASSIGNMENTS} WHERE a.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(AuthzError::AssignmentNotFound)
}

/// Removes a role grant, keeping at least one root owner. Run in system scope.
pub async fn unassign(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    actor: &Principal,
    assignment_id: Uuid,
) -> AuthzResult<()> {
    let assignment = fetch_assignment(conn, assignment_id).await?;
    let path = org_path(conn, assignment.org_id).await?;
    if !actor.can_at_path(permissions::ROLE_ASSIGN, &path) {
        return Err(AuthzError::Forbidden);
    }
    if assignment.role_key == OWNER_ROLE {
        if !actor.is_owner() {
            return Err(AuthzError::OwnerOnly);
        }
        // Serialize owner removals so two can't race past this check.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('huxley_owner_assignments'))")
            .execute(&mut *conn)
            .await?;
        let (owners,): (i64,) = sqlx::query_as(
            "SELECT count(DISTINCT a.user_id) FROM role_assignments a \
               JOIN roles r ON r.id = a.role_id \
               JOIN organizations o ON o.id = a.org_id AND o.parent_id IS NULL \
               JOIN users u ON u.id = a.user_id AND u.status = 'active' \
              WHERE r.key = 'owner' AND a.id <> $1",
        )
        .bind(assignment_id)
        .fetch_one(&mut *conn)
        .await?;
        if owners == 0 {
            return Err(AuthzError::LastOwner);
        }
    }
    sqlx::query("DELETE FROM role_assignments WHERE id = $1")
        .bind(assignment_id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.unassigned")
            .org(assignment.org_id)
            .target("user", assignment.user_id)
            .details(json!({ "role": assignment.role_key, "assignmentId": assignment_id })),
    )
    .await?;
    Ok(())
}

/// Grants Owner at the root without permission checks. For bootstrap and
/// `huxley admin create-user --owner` only. Run in system scope.
pub async fn grant_root_owner(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
) -> AuthzResult<()> {
    let (root_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations WHERE parent_id IS NULL")
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AuthzError::OrgNotFound)?;
    sqlx::query(
        "INSERT INTO role_assignments (id, user_id, role_id, org_id, inherit) \
         SELECT $1, $2, id, $3, true FROM roles WHERE key = 'owner' \
         ON CONFLICT DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(user_id)
    .bind(root_id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("role.assigned")
            .org(root_id)
            .target("user", user_id)
            .details(json!({ "role": OWNER_ROLE, "inherit": true, "break_glass": true })),
    )
    .await?;
    Ok(())
}

/// Whether disabling this user would leave no active root owner.
pub async fn is_last_owner(conn: &mut PgConnection, user_id: Uuid) -> AuthzResult<bool> {
    let (others,): (i64,) = sqlx::query_as(
        "SELECT count(DISTINCT a.user_id) FROM role_assignments a \
           JOIN roles r ON r.id = a.role_id AND r.key = 'owner' \
           JOIN organizations o ON o.id = a.org_id AND o.parent_id IS NULL \
           JOIN users u ON u.id = a.user_id AND u.status = 'active' \
          WHERE a.user_id <> $1",
    )
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await?;
    let (is_owner,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM role_assignments a \
           JOIN roles r ON r.id = a.role_id AND r.key = 'owner' \
           JOIN organizations o ON o.id = a.org_id AND o.parent_id IS NULL \
          WHERE a.user_id = $1)",
    )
    .bind(user_id)
    .fetch_one(conn)
    .await?;
    Ok(is_owner && others == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(role: &str, path: &str, inherit: bool, perms: &[&str]) -> Grant {
        Grant {
            role_key: role.to_owned(),
            org_id: Uuid::nil(),
            org_path: path.to_owned(),
            inherit,
            permissions: perms.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn principal(grants: Vec<Grant>) -> Principal {
        Principal {
            user_id: Uuid::nil(),
            grants,
            root_path: Some("root".to_owned()),
            requires_mfa: false,
        }
    }

    #[test]
    fn inherited_grants_cover_descendants_only() {
        let p = principal(vec![grant("technician", "root.msp", true, &["org:read"])]);
        assert!(p.can_at_path("org:read", "root.msp"));
        assert!(p.can_at_path("org:read", "root.msp.client"));
        assert!(!p.can_at_path("org:read", "root"), "not upward");
        assert!(
            !p.can_at_path("org:read", "root.msp2"),
            "label prefix is not ancestry"
        );
        assert!(
            !p.can_at_path("org:update", "root.msp"),
            "only granted permissions"
        );
    }

    #[test]
    fn non_inherited_grants_cover_exactly_one_org() {
        let p = principal(vec![grant("technician", "root.msp", false, &["org:read"])]);
        assert!(p.can_at_path("org:read", "root.msp"));
        assert!(!p.can_at_path("org:read", "root.msp.client"));
    }

    #[test]
    fn wildcard_and_instance_checks() {
        let admin = principal(vec![grant("admin", "root", true, &["*"])]);
        assert!(admin.can_instance("user:create"));
        assert!(admin.can_at_path("org:archive", "root.any.depth"));
        assert!(!admin.is_owner());

        let owner = principal(vec![grant(OWNER_ROLE, "root", true, &["*"])]);
        assert!(owner.is_owner());

        let scoped = principal(vec![grant("admin", "root.msp", true, &["*"])]);
        assert!(
            !scoped.can_instance("user:create"),
            "instance perms need a root grant"
        );
        assert!(scoped.granted_anywhere().contains(&"user:create"));
    }

    #[test]
    fn no_grants_means_nothing() {
        let p = principal(vec![]);
        assert!(!p.can_instance("org:read"));
        assert_eq!(p.granted_anywhere(), Vec::<&str>::new());
    }
}
