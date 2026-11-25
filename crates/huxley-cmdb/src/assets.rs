//! Assets: what a client has (devices, tenants, licenses, domains…), with
//! its type's fields, a lifecycle status, who uses it, where it is, and its
//! warranty. Each change is audited and raised as an `asset.*` event.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult, text, types};

/// Where an asset is in its life.
pub const STATUSES: &[&str] = &[
    "planned",
    "in_stock",
    "active",
    "maintenance",
    "retired",
    "disposed",
];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub type_id: Uuid,
    pub type_key: String,
    pub type_name: String,
    pub name: String,
    pub status: String,
    pub serial_number: String,
    pub hostname: String,
    pub manufacturer: String,
    pub model: String,
    pub site_id: Option<Uuid>,
    pub site_name: Option<String>,
    pub contact_id: Option<Uuid>,
    pub contact_name: Option<String>,
    pub purchased_on: Option<Date>,
    pub warranty_ends_on: Option<Date>,
    pub notes: String,
    /// The type's fields.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub attributes: sqlx::types::Json<Map<String, Json>>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub custom_fields: sqlx::types::Json<Map<String, Json>>,
    /// Ids in other systems, e.g. `{ "rmm": "…" }`.
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub external_ids: sqlx::types::Json<Map<String, Json>>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// For optimistic locking: send it back when changing the asset.
    pub version: i32,
}

const SELECT: &str = "SELECT a.id, a.org_id, o.name AS org_name, a.type_id, t.key AS type_key, t.name AS type_name, \
    a.name, a.status, a.serial_number, a.hostname, a.manufacturer, a.model, a.site_id, s.name AS site_name, \
    a.contact_id, nullif(btrim(c.first_name || ' ' || c.last_name), '') AS contact_name, a.purchased_on, \
    a.warranty_ends_on, a.notes, a.attributes, a.custom_fields, a.external_ids, a.updated_at, a.version \
    FROM assets a JOIN organizations o ON o.id = a.org_id JOIN cmdb_asset_types t ON t.id = a.type_id \
    LEFT JOIN crm_sites s ON s.id = a.site_id LEFT JOIN crm_contacts c ON c.id = a.contact_id";

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Words in the name, serial number, hostname, model or manufacturer.
    pub q: Option<String>,
    pub org_id: Option<Uuid>,
    pub type_id: Option<Uuid>,
    pub status: Option<String>,
    /// Leave out retired and disposed assets.
    pub in_use: bool,
    pub limit: i64,
    pub offset: i64,
}

/// Assets the transaction's scope can see, by name. With the total.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> CmdbResult<(Vec<Asset>, i64)> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let status = filter.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status
        && !STATUSES.contains(&s)
    {
        return Err(CmdbError::invalid("status", STATUSES.join(", ")));
    }
    let wh = "WHERE ($1::text IS NULL OR (a.name || ' ' || a.serial_number || ' ' || a.hostname || ' ' || a.model || ' ' \
                  || a.manufacturer) ILIKE '%' || $1 || '%' OR a.name % $1) \
             AND ($2::uuid IS NULL OR a.org_id = $2) \
             AND ($3::uuid IS NULL OR a.type_id = $3) \
             AND ($4::text IS NULL OR a.status = $4) \
             AND (NOT $5 OR a.status NOT IN ('retired', 'disposed'))";
    let rows: Vec<Asset> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} {wh} ORDER BY lower(a.name), a.id LIMIT $6 OFFSET $7"
    )))
    .bind(q)
    .bind(filter.org_id)
    .bind(filter.type_id)
    .bind(status)
    .bind(filter.in_use)
    .bind(filter.limit.clamp(1, 500))
    .bind(filter.offset.max(0))
    .fetch_all(&mut *conn)
    .await?;
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM assets a {wh}"
    )))
    .bind(q)
    .bind(filter.org_id)
    .bind(filter.type_id)
    .bind(status)
    .bind(filter.in_use)
    .fetch_one(conn)
    .await?;
    Ok((rows, total))
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CmdbResult<Asset> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}

/// An asset's fields, for creating it or changing it (all of them).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetInput {
    /// The client (or the MSP) it belongs to. Only for a new asset.
    #[serde(default)]
    pub org_id: Option<Uuid>,
    pub type_id: Uuid,
    pub name: String,
    /// `planned`, `in_stock`, `active` (the default), `maintenance`,
    /// `retired` or `disposed`.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub serial_number: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub manufacturer: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// A site of the asset's organization.
    #[serde(default)]
    pub site_id: Option<Uuid>,
    /// Who uses or owns it: a contact of the asset's organization.
    #[serde(default)]
    pub contact_id: Option<Uuid>,
    #[serde(default)]
    pub purchased_on: Option<Date>,
    #[serde(default)]
    pub warranty_ends_on: Option<Date>,
    #[serde(default)]
    pub notes: Option<String>,
    /// Values for the type's fields.
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub attributes: Map<String, Json>,
    #[serde(default)]
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub external_ids: Option<Map<String, Json>>,
    /// The version you read (changes only).
    #[serde(default)]
    pub version: Option<i32>,
}

struct Checked {
    name: String,
    status: String,
    serial: String,
    hostname: String,
    manufacturer: String,
    model: String,
    notes: String,
    attributes: Map<String, Json>,
    type_key: String,
}

/// Checks an asset's fields for `org_id`: its type is in use, its site and
/// contact belong to the organization, its attributes fit the type.
async fn check(conn: &mut PgConnection, org_id: Uuid, input: &AssetInput) -> CmdbResult<Checked> {
    let ty = types::get(conn, input.type_id)
        .await
        .map_err(|_| CmdbError::invalid("typeId", "there's no such asset type"))?;
    if ty.archived {
        return Err(CmdbError::invalid("typeId", "this asset type is archived"));
    }
    let status = input.status.clone().unwrap_or_else(|| "active".into());
    if !STATUSES.contains(&status.as_str()) {
        return Err(CmdbError::invalid("status", STATUSES.join(", ")));
    }
    if let Some(site) = input.site_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM crm_sites WHERE id = $1 AND org_id = $2)",
        )
        .bind(site)
        .bind(org_id)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(CmdbError::invalid(
                "siteId",
                "not a site of this organization",
            ));
        }
    }
    if let Some(contact) = input.contact_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM crm_contacts c JOIN crm_contact_companies l ON l.contact_id = c.id \
              AND l.org_id = $2 WHERE c.id = $1 AND c.archived_at IS NULL)",
        )
        .bind(contact)
        .bind(org_id)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(CmdbError::invalid(
                "contactId",
                "not a contact of this organization",
            ));
        }
    }
    if let (Some(bought), Some(ends)) = (input.purchased_on, input.warranty_ends_on)
        && ends < bought
    {
        return Err(CmdbError::invalid(
            "warrantyEndsOn",
            "after the purchase date",
        ));
    }
    if input
        .external_ids
        .as_ref()
        .is_some_and(|e| e.values().any(|v| !v.is_string()))
    {
        return Err(CmdbError::invalid("externalIds", "each id is text"));
    }
    Ok(Checked {
        name: text("name", Some(&input.name), 200)?
            .ok_or_else(|| CmdbError::invalid("name", "an asset needs a name"))?,
        status,
        serial: text("serialNumber", input.serial_number.as_deref(), 200)?.unwrap_or_default(),
        hostname: text("hostname", input.hostname.as_deref(), 255)?.unwrap_or_default(),
        manufacturer: text("manufacturer", input.manufacturer.as_deref(), 200)?.unwrap_or_default(),
        model: text("model", input.model.as_deref(), 200)?.unwrap_or_default(),
        notes: text("notes", input.notes.as_deref(), 10_000)?.unwrap_or_default(),
        attributes: ty.check(&input.attributes)?,
        type_key: ty.key,
    })
}

/// Audits and announces a change.
async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    a: &Asset,
    event: &'static str,
    details: Json,
) -> CmdbResult<()> {
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(a.org_id)
            .target("asset", a.id)
            .details(json!({ "name": a.name, "change": details })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(a.org_id),
            subject_type: "asset",
            subject_id: a.id,
            payload: json!({
                "name": a.name,
                "type": a.type_key,
                "status": a.status,
                "serialNumber": a.serial_number,
                "hostname": a.hostname,
                "actorId": ctx.actor.id(),
                "change": details,
            }),
        },
    )
    .await?;
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &AssetInput,
) -> CmdbResult<Asset> {
    let org_id = input
        .org_id
        .ok_or_else(|| CmdbError::invalid("orgId", "which organization it belongs to"))?;
    let c = check(conn, org_id, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO assets (id, org_id, type_id, name, status, serial_number, hostname, manufacturer, model, site_id, \
                contact_id, purchased_on, warranty_ends_on, notes, attributes, external_ids, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $17)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.type_id)
    .bind(&c.name)
    .bind(&c.status)
    .bind(&c.serial)
    .bind(&c.hostname)
    .bind(&c.manufacturer)
    .bind(&c.model)
    .bind(input.site_id)
    .bind(input.contact_id)
    .bind(input.purchased_on)
    .bind(input.warranty_ends_on)
    .bind(&c.notes)
    .bind(sqlx::types::Json(&c.attributes))
    .bind(sqlx::types::Json(input.external_ids.clone().unwrap_or_default()))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let asset = get(conn, id).await?;
    record(
        conn,
        ctx,
        &asset,
        "asset.created",
        json!({ "type": c.type_key }),
    )
    .await?;
    Ok(asset)
}

/// Replaces an asset's fields (send them all, with the `version` you read).
/// A change of status is raised as `asset.status_changed`.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &AssetInput,
) -> CmdbResult<Asset> {
    let before = get(conn, id).await?;
    if input.version != Some(before.version) {
        return Err(CmdbError::VersionConflict);
    }
    let c = check(conn, before.org_id, input).await?;
    let rows = sqlx::query(
        "UPDATE assets SET type_id = $3, name = $4, status = $5, serial_number = $6, hostname = $7, manufacturer = $8, \
                model = $9, site_id = $10, contact_id = $11, purchased_on = $12, warranty_ends_on = $13, notes = $14, \
                attributes = $15, external_ids = COALESCE($16, external_ids), updated_by = $17, updated_at = now(), \
                version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(before.version)
    .bind(input.type_id)
    .bind(&c.name)
    .bind(&c.status)
    .bind(&c.serial)
    .bind(&c.hostname)
    .bind(&c.manufacturer)
    .bind(&c.model)
    .bind(input.site_id)
    .bind(input.contact_id)
    .bind(input.purchased_on)
    .bind(input.warranty_ends_on)
    .bind(&c.notes)
    .bind(sqlx::types::Json(&c.attributes))
    .bind(input.external_ids.clone().map(sqlx::types::Json))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(CmdbError::VersionConflict);
    }
    let asset = get(conn, id).await?;
    if before.status == asset.status {
        record(conn, ctx, &asset, "asset.updated", json!({})).await?;
    } else {
        record(
            conn,
            ctx,
            &asset,
            "asset.status_changed",
            json!({ "from": before.status, "to": asset.status }),
        )
        .await?;
    }
    Ok(asset)
}

/// Deletes an asset and its relationships. Retiring it (its status) keeps
/// its history; deleting is for mistakes.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CmdbResult<()> {
    let before = get(conn, id).await?;
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    record(conn, ctx, &before, "asset.deleted", json!({})).await
}
