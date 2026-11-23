//! "Why can this user do that here?" (design doc §11.3: the effective
//! permissions explainer). Lists every permission with the grants that give
//! it at an org, including grants through teams.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{AuthzError, AuthzResult, PermissionScope, permissions};

/// One grant that gives a permission.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrantSource {
    pub role_name: String,
    pub org_id: Uuid,
    pub org_name: String,
    /// The grant reaches sub-organizations.
    pub inherit: bool,
    /// Set when the grant comes through a team.
    pub team_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionExplanation {
    pub key: &'static str,
    pub description: &'static str,
    pub scope: PermissionScope,
    pub allowed: bool,
    /// Grants that give it (empty when not allowed).
    pub via: Vec<GrantSource>,
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    role_name: String,
    permissions: Vec<String>,
    org_id: Uuid,
    org_name: String,
    org_path: String,
    inherit: bool,
    team_name: Option<String>,
}

fn covers(grant_path: &str, inherit: bool, path: &str) -> bool {
    path == grant_path
        || (inherit
            && path.len() > grant_path.len()
            && path.starts_with(grant_path)
            && path.as_bytes()[grant_path.len()] == b'.')
}

/// Explains `user_id`'s permissions at `org_id`. Instance-wide permissions
/// are evaluated at the root, wherever `org_id` is. Run in system scope.
pub async fn explain(
    conn: &mut PgConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> AuthzResult<Vec<PermissionExplanation>> {
    let path: Option<(String,)> =
        sqlx::query_as("SELECT path::text FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (path,) = path.ok_or(AuthzError::OrgNotFound)?;
    let (root,): (String,) =
        sqlx::query_as("SELECT path::text FROM organizations WHERE parent_id IS NULL")
            .fetch_one(&mut *conn)
            .await?;
    let sources: Vec<SourceRow> = sqlx::query_as(
        "SELECT r.name AS role_name, r.permissions, o.id AS org_id, o.name AS org_name, o.path::text AS org_path, \
                a.inherit, t.name AS team_name \
           FROM role_assignments a \
           JOIN roles r ON r.id = a.role_id \
           JOIN organizations o ON o.id = a.org_id AND o.status = 'active' \
           LEFT JOIN teams t ON t.id = a.team_id \
          WHERE a.user_id = $1 OR a.team_id IN (SELECT team_id FROM team_members WHERE user_id = $1) \
          ORDER BY o.path, r.name",
    )
    .bind(user_id)
    .fetch_all(conn)
    .await?;
    Ok(permissions::ALL
        .iter()
        .map(|p| {
            let at = if p.scope == PermissionScope::Instance {
                root.as_str()
            } else {
                path.as_str()
            };
            let via: Vec<GrantSource> = sources
                .iter()
                .filter(|s| covers(&s.org_path, s.inherit, at))
                .filter(|s| {
                    s.permissions
                        .iter()
                        .any(|x| x == permissions::WILDCARD || x == p.key)
                })
                .map(|s| GrantSource {
                    role_name: s.role_name.clone(),
                    org_id: s.org_id,
                    org_name: s.org_name.clone(),
                    inherit: s.inherit,
                    team_name: s.team_name.clone(),
                })
                .collect();
            PermissionExplanation {
                key: p.key,
                description: p.description,
                scope: p.scope,
                allowed: !via.is_empty(),
                via,
            }
        })
        .collect())
}
