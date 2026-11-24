//! Service queues: where tickets wait to be picked up. A queue belongs to an
//! organization (usually the MSP) and has members: people and teams.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    /// Tickets in it that aren't closed.
    pub open_tickets: i64,
    /// Member people's ids.
    pub user_ids: Vec<Uuid>,
    /// Member teams' ids.
    pub team_ids: Vec<Uuid>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Member people (replaces the list when given).
    #[serde(default)]
    pub user_ids: Option<Vec<Uuid>>,
    /// Member teams (replaces the list when given).
    #[serde(default)]
    pub team_ids: Option<Vec<Uuid>>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT q.id, q.org_id, q.name, q.description, q.version, q.created_at, q.archived_at, \
    (SELECT count(*) FROM sd_tickets t WHERE t.queue_id = q.id AND t.closed_at IS NULL) AS open_tickets, \
    ARRAY(SELECT m.user_id FROM sd_queue_members m WHERE m.queue_id = q.id AND m.user_id IS NOT NULL) AS user_ids, \
    ARRAY(SELECT m.team_id FROM sd_queue_members m WHERE m.queue_id = q.id AND m.team_id IS NOT NULL) AS team_ids \
    FROM sd_queues q";

/// Queues visible in the transaction's scope (optionally one organization's).
pub async fn list(
    conn: &mut PgConnection,
    org_id: Option<Uuid>,
    include_archived: bool,
) -> DeskResult<Vec<Queue>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR q.org_id = $1) AND ($2 OR q.archived_at IS NULL) \
          ORDER BY lower(q.name)"
    )))
    .bind(org_id)
    .bind(include_archived)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<Queue> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE q.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

fn name(input: &QueueInput) -> DeskResult<String> {
    text("name", Some(&input.name), 100)?
        .ok_or_else(|| DeskError::invalid("name", "a queue needs a name"))
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_queues_name") => {
            DeskError::invalid("name", "another queue there has this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

async fn set_members(
    conn: &mut PgConnection,
    queue: Uuid,
    org_id: Uuid,
    input: &QueueInput,
) -> DeskResult<()> {
    if input.user_ids.is_none() && input.team_ids.is_none() {
        return Ok(());
    }
    sqlx::query("DELETE FROM sd_queue_members WHERE queue_id = $1")
        .bind(queue)
        .execute(&mut *conn)
        .await?;
    for user in input.user_ids.iter().flatten() {
        sqlx::query(
            "INSERT INTO sd_queue_members (queue_id, org_id, user_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(queue)
        .bind(org_id)
        .bind(user)
        .execute(&mut *conn)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
                DeskError::invalid("userIds", "there's no such person")
            }
            _ => DeskError::Sqlx(e),
        })?;
    }
    for team in input.team_ids.iter().flatten() {
        sqlx::query(
            "INSERT INTO sd_queue_members (queue_id, org_id, team_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(queue)
        .bind(org_id)
        .bind(team)
        .execute(&mut *conn)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
                DeskError::invalid("teamIds", "there's no such team")
            }
            _ => DeskError::Sqlx(e),
        })?;
    }
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &QueueInput,
) -> DeskResult<Queue> {
    let name = name(input)?;
    let description = text("description", input.description.as_deref(), 2000)?.unwrap_or_default();
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO sd_queues (id, org_id, name, description) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(org_id)
        .bind(&name)
        .bind(&description)
        .execute(&mut *conn)
        .await
        .map_err(taken)?;
    set_members(conn, id, org_id, input).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("queue.created")
            .org(org_id)
            .target("queue", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &QueueInput,
) -> DeskResult<Queue> {
    let name = name(input)?;
    let description = text("description", input.description.as_deref(), 2000)?.unwrap_or_default();
    let before = get(conn, id).await?;
    let changed = sqlx::query(
        "UPDATE sd_queues SET name = $3, description = $4, version = version + 1 \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&name)
    .bind(&description)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if changed == 0 {
        return Err(DeskError::VersionConflict);
    }
    set_members(conn, id, before.org_id, input).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("queue.updated")
            .org(before.org_id)
            .target("queue", id)
            .details(json!({ "before": before.name, "after": name })),
    )
    .await?;
    get(conn, id).await
}

/// Archives a queue; its tickets keep it until they're moved.
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let queue = get(conn, id).await?;
    sqlx::query("UPDATE sd_queues SET archived_at = now(), version = version + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("queue.archived")
            .org(queue.org_id)
            .target("queue", id)
            .details(json!({ "name": queue.name })),
    )
    .await?;
    Ok(())
}
