//! Portal accounts: one per contact, at the contact's company, with a role
//! and a status. Staff (and a company's client admins) invite contacts,
//! change their role, and disable or re-enable them.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{PortalError, PortalResult, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = PortalAccountStatus)]
pub enum Status {
    /// Invited; becomes active at their first sign-in.
    Invited,
    Active,
    /// Can't sign in; their sessions end.
    Disabled,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalAccount {
    pub id: Uuid,
    pub contact_id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub email: String,
    pub role: Role,
    pub status: Status,
    pub has_password: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sign_in_at: Option<OffsetDateTime>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT: &str = "SELECT a.id, a.contact_id, a.org_id, \
    btrim(c.first_name || ' ' || c.last_name) AS name, a.email, a.role, a.status, \
    a.password_hash IS NOT NULL AS has_password, a.last_sign_in_at, a.version, a.created_at \
    FROM portal_accounts a JOIN crm_contacts c ON c.id = a.contact_id";

/// A company's portal accounts, by name.
pub async fn list(conn: &mut PgConnection, org_id: Uuid) -> PortalResult<Vec<PortalAccount>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.org_id = $1 ORDER BY lower(c.first_name || ' ' || c.last_name), a.email"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> PortalResult<PortalAccount> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(PortalError::NotFound)
}

pub async fn for_contact(
    conn: &mut PgConnection,
    contact_id: Uuid,
) -> PortalResult<Option<PortalAccount>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.contact_id = $1"
    )))
    .bind(contact_id)
    .fetch_optional(conn)
    .await?)
}

/// Gives a contact portal access with a role (or, if they have an account,
/// sets its role and re-enables it as invited). They need an email address.
pub async fn invite(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    contact_id: Uuid,
    role: Role,
) -> PortalResult<PortalAccount> {
    let contact: Option<(Uuid, Option<String>, bool)> = sqlx::query_as(
        "SELECT org_id, email, archived_at IS NOT NULL FROM crm_contacts WHERE id = $1",
    )
    .bind(contact_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((org_id, email, archived)) = contact else {
        return Err(PortalError::NotFound);
    };
    if archived {
        return Err(PortalError::invalid("contactId", "the contact is archived"));
    }
    let Some(email) = email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| e.contains('@'))
    else {
        return Err(PortalError::invalid(
            "contactId",
            "the contact needs an email address first",
        ));
    };
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM portal_accounts WHERE lower(email) = $1 AND contact_id <> $2)",
    )
    .bind(&email)
    .bind(contact_id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(PortalError::Conflict(format!(
            "another contact already has portal access as {email}"
        )));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO portal_accounts (id, contact_id, org_id, email, role, invited_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (contact_id) DO UPDATE SET role = EXCLUDED.role, email = EXCLUDED.email, \
             org_id = EXCLUDED.org_id, \
             status = CASE WHEN portal_accounts.status = 'disabled' THEN 'invited' ELSE portal_accounts.status END, \
             version = portal_accounts.version + 1, updated_at = now() \
         RETURNING id",
    )
    .bind(huxley_core::new_id())
    .bind(contact_id)
    .bind(org_id)
    .bind(&email)
    .bind(role)
    .bind(ctx.actor.id())
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.account_invited")
            .org(org_id)
            .target("portal_account", id)
            .details(json!({ "contact": contact_id, "role": role })),
    )
    .await?;
    get(conn, id).await
}

/// Changes an account's role or status (send the version you read).
/// Disabling ends its sessions.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    role: Role,
    status: Status,
    version: Option<i32>,
) -> PortalResult<PortalAccount> {
    let before = get(&mut *conn, id).await?;
    let status = match (before.status, status) {
        // Re-enabling someone who never signed in leaves them invited.
        (Status::Disabled, Status::Active) if before.last_sign_in_at.is_none() => Status::Invited,
        (Status::Invited, Status::Active) => Status::Invited,
        (_, s) => s,
    };
    let changed = sqlx::query(
        "UPDATE portal_accounts SET role = $2, status = $3, version = version + 1, updated_at = now() \
          WHERE id = $1 AND ($4::integer IS NULL OR version = $4)",
    )
    .bind(id)
    .bind(role)
    .bind(status)
    .bind(version)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(PortalError::VersionConflict);
    }
    if status == Status::Disabled {
        sqlx::query("UPDATE portal_sessions SET revoked_at = now() WHERE account_id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("portal.account_updated")
            .org(before.org_id)
            .target("portal_account", id)
            .details(json!({
                "role": role, "previousRole": before.role,
                "status": status, "previousStatus": before.status,
            })),
    )
    .await?;
    get(conn, id).await
}
