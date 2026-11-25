//! Assets linked to tickets: what a ticket is about, and an asset's ticket
//! history. A link belongs to the ticket's organization; the asset must be
//! the same client's (or the MSP's, above it).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult, assets};

/// An asset as listed on a ticket.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TicketAsset {
    pub id: Uuid,
    pub name: String,
    pub type_name: String,
    pub status: String,
    pub serial_number: String,
    pub contact_name: Option<String>,
    /// When it was linked (none for a suggestion).
    #[serde(with = "time::serde::rfc3339::option")]
    pub linked_at: Option<OffsetDateTime>,
}

const ASSET: &str = "a.id, a.name, t.name AS type_name, a.status, a.serial_number, \
    nullif(btrim(c.first_name || ' ' || c.last_name), '') AS contact_name";
const JOINS: &str =
    "JOIN cmdb_asset_types t ON t.id = a.type_id LEFT JOIN crm_contacts c ON c.id = a.contact_id";

/// The assets linked to a ticket that the scope can see.
pub async fn for_ticket(conn: &mut PgConnection, ticket: Uuid) -> CmdbResult<Vec<TicketAsset>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ASSET}, l.created_at AS linked_at FROM cmdb_asset_tickets l JOIN assets a ON a.id = l.asset_id {JOINS} \
          WHERE l.ticket_id = $1 ORDER BY l.created_at"
    )))
    .bind(ticket)
    .fetch_all(conn)
    .await?)
}

/// The requester's assets in use, not yet linked to the ticket: the likely
/// ones.
pub async fn requester_assets(
    conn: &mut PgConnection,
    ticket: Uuid,
) -> CmdbResult<Vec<TicketAsset>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ASSET}, NULL::timestamptz AS linked_at FROM sd_tickets tk JOIN assets a ON a.contact_id = tk.requester_id {JOINS} \
          WHERE tk.id = $1 AND a.status NOT IN ('retired', 'disposed') \
            AND NOT EXISTS (SELECT 1 FROM cmdb_asset_tickets l WHERE l.asset_id = a.id AND l.ticket_id = $1) \
          ORDER BY a.name LIMIT 20"
    )))
    .bind(ticket)
    .fetch_all(conn)
    .await?)
}

/// A ticket an asset is linked to.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AssetTicket {
    pub id: Uuid,
    pub reference: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// An asset's tickets that the scope can see, newest first.
pub async fn for_asset(conn: &mut PgConnection, asset: Uuid) -> CmdbResult<Vec<AssetTicket>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.number_prefix || '-' || t.number AS reference, t.title, s.name AS status_name, \
                s.category AS status_category, t.created_at \
           FROM cmdb_asset_tickets l JOIN sd_tickets t ON t.id = l.ticket_id JOIN sd_statuses s ON s.id = t.status_id \
          WHERE l.asset_id = $1 ORDER BY t.created_at DESC LIMIT 200",
    )
    .bind(asset)
    .fetch_all(conn)
    .await?)
}

/// The ticket's organization and reference (system scope).
async fn ticket(conn: &mut PgConnection, id: Uuid) -> CmdbResult<(Uuid, String)> {
    sqlx::query_as("SELECT org_id, number_prefix || '-' || number FROM sd_tickets WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CmdbError::NotFound)
}

/// Links an asset to a ticket (again is fine). The asset must belong to
/// the ticket's organization or one above it.
pub async fn link(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    asset: Uuid,
    ticket_id: Uuid,
) -> CmdbResult<()> {
    let a = assets::get(conn, asset).await?;
    let (org, reference) = ticket(conn, ticket_id).await?;
    let fits: bool = sqlx::query_scalar(
        "SELECT ao.path @> tor.path FROM organizations ao, organizations tor WHERE ao.id = $1 AND tor.id = $2",
    )
    .bind(a.org_id)
    .bind(org)
    .fetch_one(&mut *conn)
    .await?;
    if !fits {
        return Err(CmdbError::invalid(
            "assetId",
            "the asset belongs to another client",
        ));
    }
    let added = sqlx::query(
        "INSERT INTO cmdb_asset_tickets (asset_id, ticket_id, org_id, linked_by) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(asset)
    .bind(ticket_id)
    .bind(org)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if added > 0 {
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("asset.ticket_linked")
                .org(org)
                .target("asset", asset)
                .details(json!({ "name": a.name, "ticketId": ticket_id, "reference": reference })),
        )
        .await?;
    }
    Ok(())
}

/// Removes a link.
pub async fn unlink(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    asset: Uuid,
    ticket_id: Uuid,
) -> CmdbResult<()> {
    let removed: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM cmdb_asset_tickets WHERE asset_id = $1 AND ticket_id = $2 RETURNING org_id",
    )
    .bind(asset)
    .bind(ticket_id)
    .fetch_optional(&mut *conn)
    .await?;
    let org = removed.ok_or(CmdbError::NotFound)?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("asset.ticket_unlinked")
            .org(org)
            .target("asset", asset)
            .details(json!({ "ticketId": ticket_id })),
    )
    .await?;
    Ok(())
}
