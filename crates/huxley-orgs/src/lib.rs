//! Organization tree (design doc §5).
//!
//! The tree is stored as an adjacency list (`parent_id`) plus a materialized
//! `ltree` path that database triggers keep correct, including when a subtree
//! moves. Every mutation writes an audit record and a domain event in the
//! caller's transaction.
//!
//! Functions take a connection already scoped by the caller (see
//! `huxley_db::begin_scoped`): RLS hides organizations outside that scope, and
//! they behave as if they don't exist. Moves rewrite descendants' paths, so
//! [`move_to`] must run in system scope after the caller is authorized.

pub mod settings;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_db::{DbError, violated_constraint};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

/// `provider`: the owning company, an MSP brand, or a division. `client`: a
/// customer of a provider, or a site/sub-org of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "org_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OrgKind {
    Provider,
    Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "org_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OrgStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    /// Depth in the tree; the root is 1.
    pub depth: i32,
    pub kind: OrgKind,
    pub name: String,
    pub status: OrgStatus,
    /// Optimistic concurrency token. Send it back when updating.
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum OrgError {
    #[error("organization not found")]
    NotFound,
    #[error("an active sibling already uses this name")]
    NameTaken,
    #[error("the instance already has a root organization")]
    RootExists,
    #[error("a provider organization cannot be placed under a client")]
    ProviderUnderClient,
    #[error("an organization cannot be moved under itself or its descendants")]
    Cycle,
    #[error("the root organization cannot be moved or archived")]
    RootImmutable,
    #[error("archive or move its active sub-organizations first")]
    HasActiveChildren,
    #[error("the organization was changed by someone else; reload and retry")]
    VersionConflict,
    #[error("invalid name: {0}")]
    InvalidName(&'static str),
    #[error("unknown setting")]
    UnknownSetting,
    #[error("{0}")]
    InvalidSetting(String),
    #[error("locked by {0}")]
    SettingLocked(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for OrgError {
    fn from(error: sqlx::Error) -> Self {
        match violated_constraint(&error) {
            Some("organizations_sibling_name") => Self::NameTaken,
            Some("organizations_single_root") => Self::RootExists,
            Some("organizations_provider_under_client") => Self::ProviderUnderClient,
            Some("organizations_no_cycles" | "organizations_not_own_parent") => Self::Cycle,
            _ => Self::Db(DbError::Sqlx(error)),
        }
    }
}

pub type OrgResult<T> = Result<T, OrgError>;

/// Builds a static query over `organizations` with the standard column list.
/// sqlx requires static SQL; `concat!` keeps it static.
macro_rules! select_orgs {
    ($rest:literal) => {
        concat!(
            "SELECT id, parent_id, nlevel(path) AS depth, kind, name, status, ",
            "version, created_at, updated_at FROM organizations ",
            $rest
        )
    };
}

const MAX_NAME_LEN: usize = 200;

fn normalize_name(name: &str) -> OrgResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::InvalidName("must not be empty"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(OrgError::InvalidName("must be at most 200 characters"));
    }
    if name.chars().any(char::is_control) {
        return Err(OrgError::InvalidName("must not contain control characters"));
    }
    Ok(name.to_owned())
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> OrgResult<Organization> {
    sqlx::query_as(select_orgs!("WHERE id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(OrgError::NotFound)
}

/// The root organization, if it exists and is in scope.
pub async fn root(conn: &mut PgConnection) -> OrgResult<Option<Organization>> {
    Ok(sqlx::query_as(select_orgs!("WHERE parent_id IS NULL"))
        .fetch_optional(conn)
        .await?)
}

/// Direct children, ordered by name.
pub async fn children(conn: &mut PgConnection, parent_id: Uuid) -> OrgResult<Vec<Organization>> {
    Ok(sqlx::query_as(select_orgs!(
        "WHERE parent_id = $1 ORDER BY lower(name), id"
    ))
    .bind(parent_id)
    .fetch_all(conn)
    .await?)
}

/// The org and all its descendants that are in scope, ordered by path.
pub async fn subtree(conn: &mut PgConnection, id: Uuid) -> OrgResult<Vec<Organization>> {
    Ok(sqlx::query_as(select_orgs!(
        "WHERE path <@ (SELECT path FROM organizations WHERE id = $1) ORDER BY path"
    ))
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Ancestors from the root down to (and including) the org itself. Used for
/// inheritance: settings resolve from the nearest ancestor that defines them.
pub async fn ancestors(conn: &mut PgConnection, id: Uuid) -> OrgResult<Vec<Organization>> {
    Ok(sqlx::query_as(select_orgs!(
        "WHERE path @> (SELECT path FROM organizations WHERE id = $1) ORDER BY nlevel(path)"
    ))
    .bind(id)
    .fetch_all(conn)
    .await?)
}

/// Creates the root organization. Run once, in system scope, at bootstrap.
pub async fn create_root(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    name: &str,
) -> OrgResult<Organization> {
    let name = normalize_name(name)?;
    let id = huxley_core::new_id();
    // `path` is computed by the insert trigger; the placeholder is overwritten.
    sqlx::query(
        "INSERT INTO organizations (id, kind, name, path, created_by, updated_by) \
         VALUES ($1, 'provider', $2, '', $3, $3)",
    )
    .bind(id)
    .bind(&name)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    huxley_db::extend_scope(conn, id).await?;
    after_create(conn, ctx, id).await
}

/// Creates an organization under `parent_id`. The parent must be in scope.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    parent_id: Uuid,
    kind: OrgKind,
    name: &str,
) -> OrgResult<Organization> {
    let name = normalize_name(name)?;
    // Visible-parent check: RLS hides out-of-scope parents, so they read as missing.
    let parent = get(conn, parent_id).await?;
    if parent.status == OrgStatus::Archived {
        return Err(OrgError::NotFound);
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO organizations (id, parent_id, kind, name, path, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, '', $5, $5)",
    )
    .bind(id)
    .bind(parent_id)
    .bind(kind)
    .bind(&name)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    huxley_db::extend_scope(conn, id).await?;
    after_create(conn, ctx, id).await
}

async fn after_create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> OrgResult<Organization> {
    let org = get(conn, id).await?;
    let snapshot = json!({ "name": org.name, "kind": org.kind, "parentId": org.parent_id });
    audit::record(
        conn,
        ctx,
        AuditRecord::new("org.created")
            .org(id)
            .target("organization", id)
            .details(json!({ "after": snapshot })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "org.created",
            org_id: Some(id),
            subject_type: "organization",
            subject_id: id,
            payload: snapshot,
        },
    )
    .await?;
    Ok(org)
}

/// Renames an organization.
pub async fn rename(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    expected_version: i32,
    name: &str,
) -> OrgResult<Organization> {
    let name = normalize_name(name)?;
    let before = get(conn, id).await?;
    if before.version != expected_version {
        return Err(OrgError::VersionConflict);
    }
    let updated = sqlx::query(
        "UPDATE organizations SET name = $3, version = version + 1, updated_by = $4 \
         WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(expected_version)
    .bind(&name)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(OrgError::VersionConflict);
    }
    let after = get(conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("org.renamed")
            .org(id)
            .target("organization", id)
            .details(json!({ "before": { "name": before.name }, "after": { "name": after.name } })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "org.updated",
            org_id: Some(id),
            subject_type: "organization",
            subject_id: id,
            payload: json!({ "name": after.name }),
        },
    )
    .await?;
    Ok(after)
}

/// Moves an organization (and its whole subtree) under a new parent.
///
/// Must run in system scope: the path trigger rewrites every descendant, and
/// RLS would hide those outside the caller's scope. Authorize first (the
/// caller needs rights on the org, its current parent, and the new parent).
pub async fn move_to(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    expected_version: i32,
    new_parent_id: Uuid,
) -> OrgResult<Organization> {
    let before = get(conn, id).await?;
    if before.parent_id.is_none() {
        return Err(OrgError::RootImmutable);
    }
    if before.version != expected_version {
        return Err(OrgError::VersionConflict);
    }
    let new_parent = get(conn, new_parent_id).await?;
    if new_parent.status == OrgStatus::Archived {
        return Err(OrgError::NotFound);
    }
    if before.parent_id == Some(new_parent_id) {
        return Ok(before);
    }
    let updated = sqlx::query(
        "UPDATE organizations SET parent_id = $3, version = version + 1, updated_by = $4 \
         WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(expected_version)
    .bind(new_parent_id)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(OrgError::VersionConflict);
    }
    let after = get(conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("org.moved")
            .org(id)
            .target("organization", id)
            .details(json!({
                "before": { "parentId": before.parent_id },
                "after": { "parentId": after.parent_id },
            })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "org.moved",
            org_id: Some(id),
            subject_type: "organization",
            subject_id: id,
            payload: json!({ "fromParentId": before.parent_id, "toParentId": after.parent_id }),
        },
    )
    .await?;
    Ok(after)
}

/// Archives an organization. It must have no active sub-organizations.
pub async fn archive(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    expected_version: i32,
) -> OrgResult<Organization> {
    let before = get(conn, id).await?;
    if before.parent_id.is_none() {
        return Err(OrgError::RootImmutable);
    }
    if before.version != expected_version {
        return Err(OrgError::VersionConflict);
    }
    let (active_children,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM organizations WHERE parent_id = $1 AND status = 'active'",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if active_children > 0 {
        return Err(OrgError::HasActiveChildren);
    }
    let updated = sqlx::query(
        "UPDATE organizations SET status = 'archived', version = version + 1, updated_by = $3 \
         WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(expected_version)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(OrgError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("org.archived")
            .org(id)
            .target("organization", id),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "org.archived",
            org_id: Some(id),
            subject_type: "organization",
            subject_id: id,
            payload: json!({}),
        },
    )
    .await?;
    get(conn, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_validated() {
        assert_eq!(normalize_name("  Contoso  ").unwrap(), "Contoso");
        assert!(matches!(
            normalize_name("   "),
            Err(OrgError::InvalidName(_))
        ));
        assert!(matches!(
            normalize_name(&"x".repeat(201)),
            Err(OrgError::InvalidName(_))
        ));
        assert!(matches!(
            normalize_name("bad\u{0}name"),
            Err(OrgError::InvalidName(_))
        ));
        assert!(normalize_name(&"é".repeat(200)).is_ok());
    }
}
