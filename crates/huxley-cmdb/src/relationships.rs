//! Typed relationships between assets, and impact analysis.
//!
//! An edge reads `from <kind> to`: a VM `runs_on` a host, an app
//! `depends_on` a database server, a host `hosts` a VM, a workstation is
//! `licensed_by` a license, a NAS `backs_up` a server, a switch
//! `connects_to` a firewall.
//!
//! **Impact** of an asset going down: what runs on it, depends on it, is
//! licensed by it, or is hosted by it, and so on transitively. Backups and
//! network links don't carry impact (a down backup doesn't stop what it
//! backs up; links are usually redundant).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CmdbError, CmdbResult, assets};

/// The kinds of relationship.
pub const KINDS: &[&str] = &[
    "runs_on",
    "depends_on",
    "connects_to",
    "hosts",
    "licensed_by",
    "backs_up",
];

/// A relationship as seen from one asset.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssetRelationship)]
pub struct Relationship {
    pub id: Uuid,
    pub kind: String,
    /// `out`: this asset `kind` the other; `in`: the other `kind` this one.
    pub direction: String,
    pub other_id: Uuid,
    pub other_name: String,
    pub other_type: String,
    pub other_status: String,
}

/// An asset's relationships, both ways, that the scope can see.
pub async fn for_asset(conn: &mut PgConnection, id: Uuid) -> CmdbResult<Vec<Relationship>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.kind, 'out' AS direction, o.id AS other_id, o.name AS other_name, t.name AS other_type, \
                o.status AS other_status \
           FROM cmdb_relationships r JOIN assets o ON o.id = r.to_id JOIN cmdb_asset_types t ON t.id = o.type_id \
          WHERE r.from_id = $1 \
         UNION ALL \
         SELECT r.id, r.kind, 'in', o.id, o.name, t.name, o.status \
           FROM cmdb_relationships r JOIN assets o ON o.id = r.from_id JOIN cmdb_asset_types t ON t.id = o.type_id \
          WHERE r.to_id = $1 \
          ORDER BY 2, 5",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssetRelationshipInput)]
pub struct RelationshipInput {
    pub kind: String,
    /// The other asset: this one `kind` it.
    pub to_id: Uuid,
}

/// Adds `from kind to`. Both must be in the same organization tree (one
/// above or below the other, or the same). Adding it again is fine.
pub async fn add(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    from: Uuid,
    input: &RelationshipInput,
) -> CmdbResult<()> {
    if !KINDS.contains(&input.kind.as_str()) {
        return Err(CmdbError::invalid("kind", KINDS.join(", ")));
    }
    if from == input.to_id {
        return Err(CmdbError::invalid(
            "toId",
            "an asset can't relate to itself",
        ));
    }
    let a = assets::get(conn, from).await?;
    let b = assets::get(conn, input.to_id)
        .await
        .map_err(|_| CmdbError::invalid("toId", "there's no such asset"))?;
    let related: bool = sqlx::query_scalar(
        "SELECT oa.path @> ob.path OR ob.path @> oa.path FROM organizations oa, organizations ob \
          WHERE oa.id = $1 AND ob.id = $2",
    )
    .bind(a.org_id)
    .bind(b.org_id)
    .fetch_one(&mut *conn)
    .await?;
    if !related {
        return Err(CmdbError::invalid(
            "toId",
            "the assets belong to unrelated organizations",
        ));
    }
    let added = sqlx::query(
        "INSERT INTO cmdb_relationships (id, org_id, from_id, to_id, kind, created_by) VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (from_id, to_id, kind) DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(a.org_id)
    .bind(from)
    .bind(input.to_id)
    .bind(&input.kind)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if added > 0 {
        audit::record(
            &mut *conn,
            ctx,
            AuditRecord::new("asset.related")
                .org(a.org_id)
                .target("asset", from)
                .details(json!({ "name": a.name, "kind": input.kind, "to": b.name, "toId": b.id })),
        )
        .await?;
    }
    Ok(())
}

/// Removes a relationship.
pub async fn remove(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> CmdbResult<()> {
    let row: Option<(Uuid, Uuid, String, Uuid)> = sqlx::query_as(
        "DELETE FROM cmdb_relationships WHERE id = $1 RETURNING org_id, from_id, kind, to_id",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (org, from, kind, to) = row.ok_or(CmdbError::NotFound)?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("asset.unrelated")
            .org(org)
            .target("asset", from)
            .details(json!({ "kind": kind, "toId": to })),
    )
    .await?;
    Ok(())
}

/// An asset affected when another goes down.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Impacted {
    pub id: Uuid,
    pub name: String,
    pub type_name: String,
    pub status: String,
    pub org_name: String,
    /// How many steps away (1: directly).
    pub depth: i32,
    /// The asset it's affected through (the one it runs on, depends on…).
    pub via_id: Uuid,
    pub via_name: String,
    pub via_kind: String,
}

/// What breaks if an asset goes down: everything that (transitively) runs
/// on it, depends on it, is licensed by it, or is hosted by it, nearest
/// first, up to `max_depth` steps. Retired and disposed assets are left
/// out. What the scope can see.
pub async fn impact(
    conn: &mut PgConnection,
    id: Uuid,
    max_depth: i32,
) -> CmdbResult<Vec<Impacted>> {
    assets::get(conn, id).await?;
    Ok(sqlx::query_as(
        "WITH RECURSIVE edges AS ( \
            SELECT from_id AS dependent, to_id AS dependency, kind FROM cmdb_relationships \
             WHERE kind IN ('runs_on', 'depends_on', 'licensed_by') \
            UNION ALL \
            SELECT to_id, from_id, kind FROM cmdb_relationships WHERE kind = 'hosts'), \
         hit (id, depth, via, kind, path) AS ( \
            SELECT e.dependent, 1, e.dependency, e.kind, ARRAY[e.dependency, e.dependent] \
              FROM edges e WHERE e.dependency = $1 \
            UNION ALL \
            SELECT e.dependent, h.depth + 1, e.dependency, e.kind, h.path || e.dependent \
              FROM hit h JOIN edges e ON e.dependency = h.id \
             WHERE h.depth < $2 AND NOT e.dependent = ANY(h.path)), \
         nearest AS ( \
            SELECT DISTINCT ON (id) id, depth, via, kind FROM hit ORDER BY id, depth) \
         SELECT a.id, a.name, t.name AS type_name, a.status, o.name AS org_name, n.depth, \
                n.via AS via_id, v.name AS via_name, n.kind AS via_kind \
           FROM nearest n \
           JOIN assets a ON a.id = n.id \
           JOIN cmdb_asset_types t ON t.id = a.type_id \
           JOIN organizations o ON o.id = a.org_id \
           JOIN assets v ON v.id = n.via \
          WHERE a.status NOT IN ('retired', 'disposed') \
          ORDER BY n.depth, a.name",
    )
    .bind(id)
    .bind(max_depth.clamp(1, 20))
    .fetch_all(conn)
    .await?)
}

/// An asset on a dependency map.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MapNode {
    pub id: Uuid,
    pub name: String,
    pub type_key: String,
    pub type_name: String,
    pub status: String,
    /// Steps from the asset the map is of (0: that asset).
    pub depth: i32,
}

/// A relationship on a dependency map: `from kind to`.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MapEdge {
    pub id: Uuid,
    pub from_id: Uuid,
    pub to_id: Uuid,
    pub kind: String,
}

/// The neighborhood of an asset, for its dependency map.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DependencyMap {
    pub nodes: Vec<MapNode>,
    pub edges: Vec<MapEdge>,
    /// Whether nodes were left out (the map stops at `MAP_LIMIT`).
    pub truncated: bool,
}

/// The most assets on a map.
pub const MAP_LIMIT: i64 = 150;

/// The assets within `depth` relationships of an asset (either way), and
/// the relationships between them: what the scope can see.
pub async fn map(conn: &mut PgConnection, id: Uuid, depth: i32) -> CmdbResult<DependencyMap> {
    assets::get(conn, id).await?;
    let mut nodes: Vec<MapNode> = sqlx::query_as(
        "WITH RECURSIVE links AS ( \
            SELECT from_id AS a, to_id AS b FROM cmdb_relationships \
            UNION ALL SELECT to_id, from_id FROM cmdb_relationships), \
         near (id, depth) AS ( \
            SELECT $1::uuid, 0 \
            UNION \
            SELECT l.b, n.depth + 1 FROM near n JOIN links l ON l.a = n.id WHERE n.depth < $2), \
         nearest AS (SELECT id, min(depth) AS depth FROM near GROUP BY id) \
         SELECT a.id, a.name, t.key AS type_key, t.name AS type_name, a.status, n.depth \
           FROM nearest n JOIN assets a ON a.id = n.id JOIN cmdb_asset_types t ON t.id = a.type_id \
          ORDER BY n.depth, a.name LIMIT $3",
    )
    .bind(id)
    .bind(depth.clamp(1, 4))
    .bind(MAP_LIMIT + 1)
    .fetch_all(&mut *conn)
    .await?;
    let truncated = nodes.len() > usize::try_from(MAP_LIMIT).unwrap_or(150);
    nodes.truncate(usize::try_from(MAP_LIMIT).unwrap_or(150));
    let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
    let edges = sqlx::query_as(
        "SELECT id, from_id, to_id, kind FROM cmdb_relationships WHERE from_id = ANY($1) AND to_id = ANY($1) ORDER BY kind, id",
    )
    .bind(&ids)
    .fetch_all(conn)
    .await?;
    Ok(DependencyMap {
        nodes,
        edges,
        truncated,
    })
}
