//! Which users must have a second factor (design doc §11.2).
//!
//! Policy lives in two flags: `roles.require_mfa` and
//! `organizations.require_mfa`. A user is covered if they hold a role with the
//! flag (at any org, directly or through a team), or can reach an org with
//! the flag: assigned at it or a
//! descendant of it, or at an ancestor with inheritance. The root org's flag
//! covers everyone, with or without roles.
//!
//! The API refuses everything except setting up a second factor to covered
//! users who don't have one yet.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{AuthzError, AuthzResult, role_by_id};

/// Whether policy requires `user_id` to have a second factor. Run in system
/// scope.
pub async fn user_requires_mfa(conn: &mut PgConnection, user_id: Uuid) -> AuthzResult<bool> {
    let (required,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM organizations WHERE parent_id IS NULL AND require_mfa) \
             OR EXISTS ( \
                SELECT 1 FROM role_assignments a \
                  JOIN roles r ON r.id = a.role_id \
                  JOIN organizations o ON o.id = a.org_id AND o.status = 'active' \
                 WHERE (a.user_id = $1 OR a.team_id IN \
                        (SELECT team_id FROM team_members WHERE user_id = $1)) \
                   AND (r.require_mfa OR EXISTS ( \
                        SELECT 1 FROM organizations p \
                         WHERE p.require_mfa AND p.status = 'active' \
                           AND (p.path @> o.path OR (a.inherit AND o.path @> p.path)))))",
    )
    .bind(user_id)
    .fetch_one(conn)
    .await?;
    Ok(required)
}

/// An organization that requires MFA.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PolicyOrg {
    pub id: Uuid,
    pub name: String,
    /// The root: the requirement covers every user.
    pub is_root: bool,
}

/// The whole MFA policy at a glance.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaPolicy {
    /// Every user must have a second factor (set on the root organization).
    pub everyone: bool,
    /// Roles whose holders must have a second factor.
    pub role_ids: Vec<Uuid>,
    /// Organizations whose users must have a second factor.
    pub orgs: Vec<PolicyOrg>,
}

pub async fn get(conn: &mut PgConnection) -> AuthzResult<MfaPolicy> {
    let role_ids: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM roles WHERE require_mfa ORDER BY name")
            .fetch_all(&mut *conn)
            .await?;
    let orgs: Vec<PolicyOrg> = sqlx::query_as(
        "SELECT id, name, parent_id IS NULL AS is_root FROM organizations \
          WHERE require_mfa AND status = 'active' ORDER BY path",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(MfaPolicy {
        everyone: orgs.iter().any(|o| o.is_root),
        role_ids: role_ids.into_iter().map(|(id,)| id).collect(),
        orgs,
    })
}

/// Requires (or stops requiring) MFA for holders of a role. Run in system
/// scope after checking `security:policy:manage`.
pub async fn set_role(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    role_id: Uuid,
    required: bool,
) -> AuthzResult<()> {
    let role = role_by_id(conn, role_id).await?;
    if role.require_mfa == required {
        return Ok(());
    }
    sqlx::query("UPDATE roles SET require_mfa = $2 WHERE id = $1")
        .bind(role_id)
        .bind(required)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("security.mfa_policy_changed")
            .target("role", role_id)
            .details(json!({ "role": role.key, "required": required })),
    )
    .await?;
    Ok(())
}

/// Requires (or stops requiring) MFA for users who can reach an
/// organization. On the root, that's everyone. Run in system scope after
/// checking `security:policy:manage`.
pub async fn set_org(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    required: bool,
) -> AuthzResult<()> {
    let row: Option<(bool, String)> =
        sqlx::query_as("SELECT require_mfa, name FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (current, name) = row.ok_or(AuthzError::OrgNotFound)?;
    if current == required {
        return Ok(());
    }
    sqlx::query("UPDATE organizations SET require_mfa = $2 WHERE id = $1")
        .bind(org_id)
        .bind(required)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("security.mfa_policy_changed")
            .org(org_id)
            .target("organization", org_id)
            .details(json!({ "organization": name, "required": required })),
    )
    .await?;
    Ok(())
}
