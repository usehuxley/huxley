//! Devices in the client portal (ADR-040): a company's assets in use, as a
//! client sees them. Client admins see the company's; everyone sees those
//! assigned to them. No notes, custom fields, outside ids or sync details.

use serde::Serialize;
use sqlx::PgConnection;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult};

/// Which devices a portal account sees.
#[derive(Debug, Clone, Copy)]
pub enum Seen {
    /// The company's.
    Company,
    /// Those assigned to this contact.
    Own(Uuid),
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = PortalDevice)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub type_name: String,
    pub status: String,
    pub serial_number: String,
    pub hostname: String,
    pub manufacturer: String,
    pub model: String,
    pub site_name: Option<String>,
    pub contact_id: Option<Uuid>,
    pub contact_name: Option<String>,
    #[serde(with = "crate::date::option")]
    #[schema(value_type = Option<String>, format = Date)]
    pub warranty_ends_on: Option<Date>,
}

const SELECT: &str = "SELECT a.id, a.name, t.name AS type_name, a.status, a.serial_number, a.hostname, \
    a.manufacturer, a.model, s.name AS site_name, a.contact_id, \
    nullif(btrim(c.first_name || ' ' || c.last_name), '') AS contact_name, a.warranty_ends_on \
    FROM assets a JOIN cmdb_asset_types t ON t.id = a.type_id \
    LEFT JOIN crm_sites s ON s.id = a.site_id LEFT JOIN crm_contacts c ON c.id = a.contact_id \
    WHERE a.org_id = $1 AND a.status NOT IN ('retired', 'disposed') \
      AND ($2::uuid IS NULL OR a.contact_id = $2)";

fn own(seen: Seen) -> Option<Uuid> {
    match seen {
        Seen::Company => None,
        Seen::Own(contact) => Some(contact),
    }
}

/// A company's devices in use (or a contact's own), by name.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Uuid,
    seen: Seen,
    q: Option<&str>,
) -> CmdbResult<Vec<Device>> {
    let q = q.map(str::trim).filter(|q| !q.is_empty());
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} AND ($3::text IS NULL OR (a.name || ' ' || a.serial_number || ' ' || a.hostname || ' ' \
                    || a.model || ' ' || a.manufacturer) ILIKE '%' || $3 || '%') \
         ORDER BY lower(a.name), a.id LIMIT 500"
    )))
    .bind(org_id)
    .bind(own(seen))
    .bind(q)
    .fetch_all(conn)
    .await?)
}

/// One device, if the account sees it.
pub async fn get(
    conn: &mut PgConnection,
    org_id: Uuid,
    seen: Seen,
    id: Uuid,
) -> CmdbResult<Device> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND a.id = $3")))
        .bind(org_id)
        .bind(own(seen))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}
