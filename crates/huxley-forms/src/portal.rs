//! Forms in the client portal (ADR-040): those offered to the contacts of
//! the form's organization and the organizations below it, and what a
//! contact has asked for with them.

use serde::Serialize;
use serde_json::Value as Json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{FormError, FormResult};

/// A form as the portal shows it.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalForm)]
pub struct CatalogForm {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub schema: Json,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub ui: Json,
}

const SELECT: &str = "SELECT f.id, f.name, f.description, f.portal_category AS category, f.schema, f.ui \
    FROM forms f JOIN organizations o ON o.id = f.org_id \
    WHERE f.portal AND NOT f.archived AND (SELECT path FROM organizations WHERE id = $1) <@ o.path";

/// The forms offered to a company's contacts: its own and those of the
/// organizations above it, by category and name.
pub async fn catalog(conn: &mut PgConnection, org_id: Uuid) -> FormResult<Vec<CatalogForm>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY f.portal_category NULLS LAST, lower(f.name), f.id"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

/// One form, if it's offered to the company's contacts.
pub async fn get(conn: &mut PgConnection, org_id: Uuid, id: Uuid) -> FormResult<CatalogForm> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND f.id = $2")))
        .bind(org_id)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(FormError::NotFound)
}

/// Something a contact asked for in the portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalRequest)]
pub struct Request {
    /// The submission.
    pub id: Uuid,
    pub form_id: Uuid,
    pub form_name: String,
    /// `received` (nothing to run), `in_progress`, `done`, or `failed` (a
    /// run failed or was cancelled; the service desk can see why).
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A portal account's requests, newest first.
pub async fn requests(conn: &mut PgConnection, account: Uuid) -> FormResult<Vec<Request>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.form_id, f.name AS form_name, s.created_at, \
                CASE WHEN cardinality(s.run_ids) = 0 THEN 'received' \
                     WHEN EXISTS (SELECT 1 FROM runs r WHERE r.id = ANY (s.run_ids) \
                                     AND r.status IN ('failed', 'cancelled')) THEN 'failed' \
                     WHEN EXISTS (SELECT 1 FROM runs r WHERE r.id = ANY (s.run_ids) \
                                     AND r.status IN ('queued', 'running')) THEN 'in_progress' \
                     ELSE 'done' END AS status \
           FROM form_submissions s JOIN forms f ON f.id = s.form_id \
          WHERE s.portal_account_id = $1 ORDER BY s.created_at DESC LIMIT 100",
    )
    .bind(account)
    .fetch_all(conn)
    .await?)
}
