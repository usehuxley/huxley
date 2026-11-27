//! Where stock is kept: warehouses, bins in them, technicians' vehicles
//! and client sites.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{InventoryError, InventoryResult, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = InventoryLocationKind)]
pub enum Kind {
    Warehouse,
    /// A shelf or bin in a warehouse.
    Bin,
    /// A technician's vehicle.
    Vehicle,
    /// Stock kept at a client's site.
    Site,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = InventoryLocation)]
pub struct Location {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub kind: Kind,
    pub parent_id: Option<Uuid>,
    pub parent_name: Option<String>,
    pub assigned_user_id: Option<Uuid>,
    pub assigned_user_name: Option<String>,
    pub address: Option<String>,
    pub active: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = InventoryLocationInput)]
pub struct LocationInput {
    pub name: String,
    pub kind: Kind,
    /// A bin's warehouse.
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// A vehicle's technician.
    #[serde(default)]
    pub assigned_user_id: Option<Uuid>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default = "yes")]
    pub active: bool,
}

const fn yes() -> bool {
    true
}

const SELECT: &str = "SELECT l.id, l.org_id, l.name, l.kind, l.parent_id, p.name AS parent_name, \
        l.assigned_user_id, u.display_name AS assigned_user_name, l.address, l.active, l.created_at \
   FROM inventory_locations l LEFT JOIN inventory_locations p ON p.id = l.parent_id \
   LEFT JOIN users u ON u.id = l.assigned_user_id";

/// Locations you can see, by organization and name.
pub async fn list(
    conn: &mut PgConnection,
    include_inactive: bool,
) -> InventoryResult<Vec<Location>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE $1 OR l.active ORDER BY l.org_id, coalesce(p.name, l.name), l.parent_id NULLS FIRST, l.name"
    )))
    .bind(include_inactive)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Location> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE l.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(InventoryError::NotFound)
}

/// Where a location is (for checking access there).
pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> InventoryResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM inventory_locations WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

struct Checked {
    name: String,
    address: Option<String>,
    parent: Option<Uuid>,
    user: Option<Uuid>,
}

async fn check(
    conn: &mut PgConnection,
    org_id: Uuid,
    id: Option<Uuid>,
    input: &LocationInput,
) -> InventoryResult<Checked> {
    let name = required("name", &input.name, 100)?;
    let address = text("address", input.address.as_deref(), 500)?;
    let parent = match (input.kind, input.parent_id) {
        (Kind::Bin, None) => {
            return Err(InventoryError::invalid(
                "parentId",
                "a bin is in a warehouse",
            ));
        }
        (Kind::Bin, Some(p)) => {
            let ok: Option<(Uuid, Kind)> =
                sqlx::query_as("SELECT org_id, kind FROM inventory_locations WHERE id = $1")
                    .bind(p)
                    .fetch_optional(&mut *conn)
                    .await?;
            if Some(p) == id || !matches!(ok, Some((o, Kind::Warehouse)) if o == org_id) {
                return Err(InventoryError::invalid(
                    "parentId",
                    "not a warehouse of this organization",
                ));
            }
            Some(p)
        }
        _ => None,
    };
    let user = if input.kind == Kind::Vehicle {
        input.assigned_user_id
    } else {
        None
    };
    if let Some(u) = user {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND status = 'active')",
        )
        .bind(u)
        .fetch_one(&mut *conn)
        .await?;
        if !found {
            return Err(InventoryError::invalid("assignedUserId", "no such person"));
        }
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM inventory_locations WHERE org_id = $1 AND lower(name) = lower($2) \
            AND id IS DISTINCT FROM $3)",
    )
    .bind(org_id)
    .bind(&name)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Err(InventoryError::invalid(
            "name",
            "another location has that name",
        ));
    }
    Ok(Checked {
        name,
        address,
        parent,
        user,
    })
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &LocationInput,
) -> InventoryResult<Location> {
    let c = check(&mut *conn, org_id, None, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO inventory_locations (id, org_id, name, kind, parent_id, assigned_user_id, address, active) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(input.kind)
    .bind(c.parent)
    .bind(c.user)
    .bind(&c.address)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("inventory_location.created")
            .org(org_id)
            .target("inventory_location", id)
            .details(json!({ "name": c.name, "kind": input.kind })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &LocationInput,
) -> InventoryResult<Location> {
    let current = get(&mut *conn, id).await?;
    let c = check(&mut *conn, current.org_id, Some(id), input).await?;
    if current.kind != input.kind && current.kind == Kind::Warehouse {
        let bins: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM inventory_locations WHERE parent_id = $1)",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if bins {
            return Err(InventoryError::Conflict(
                "it has bins; move or remove them first".into(),
            ));
        }
    }
    sqlx::query(
        "UPDATE inventory_locations SET name = $2, kind = $3, parent_id = $4, assigned_user_id = $5, \
            address = $6, active = $7 WHERE id = $1",
    )
    .bind(id)
    .bind(&c.name)
    .bind(input.kind)
    .bind(c.parent)
    .bind(c.user)
    .bind(&c.address)
    .bind(input.active)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("inventory_location.updated")
            .org(current.org_id)
            .target("inventory_location", id)
            .details(json!({ "name": c.name, "kind": input.kind, "active": input.active })),
    )
    .await?;
    get(conn, id).await
}

/// Removes an empty location (nothing on hand, no bins, never used); one
/// that was used can be turned off instead.
pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> InventoryResult<()> {
    let current = get(&mut *conn, id).await?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM inventory_stock WHERE location_id = $1) \
             OR EXISTS (SELECT 1 FROM inventory_locations WHERE parent_id = $1) \
             OR EXISTS (SELECT 1 FROM inventory_movements WHERE from_location = $1 OR to_location = $1)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if used {
        return Err(InventoryError::Conflict(
            "it has stock, bins or history; turn it off instead".into(),
        ));
    }
    sqlx::query("DELETE FROM inventory_levels WHERE location_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM inventory_locations WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("inventory_location.deleted")
            .org(current.org_id)
            .target("inventory_location", id)
            .details(json!({ "name": current.name })),
    )
    .await?;
    Ok(())
}
