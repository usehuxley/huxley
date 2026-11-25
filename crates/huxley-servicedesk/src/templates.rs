//! Ticket templates (design doc §8.2): a common ticket's type, title,
//! description, impact, urgency and queue, to start new tickets from.
//! Instance-wide.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TicketTemplate {
    pub id: Uuid,
    pub name: String,
    /// The ticket type's key, e.g. `service_request`.
    pub type_key: String,
    pub title: String,
    pub description: String,
    /// 1 (high) to 3 (low).
    pub impact: i16,
    /// 1 (high) to 3 (low).
    pub urgency: i16,
    pub queue_id: Option<Uuid>,
    pub queue_name: Option<String>,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TicketTemplateInput {
    pub name: String,
    /// The ticket type's key (default `incident`).
    #[serde(default)]
    pub type_key: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub impact: Option<i16>,
    #[serde(default)]
    pub urgency: Option<i16>,
    #[serde(default)]
    pub queue_id: Option<Uuid>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT t.id, t.name, t.type_key, t.title, t.description, t.impact, t.urgency, \
    t.queue_id, q.name AS queue_name, t.version \
    FROM sd_ticket_templates t LEFT JOIN sd_queues q ON q.id = t.queue_id AND q.archived_at IS NULL \
    WHERE t.archived_at IS NULL";

pub async fn list(conn: &mut PgConnection) -> DeskResult<Vec<TicketTemplate>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY lower(t.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<TicketTemplate> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND t.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

struct Checked {
    name: String,
    type_key: String,
    title: String,
    description: String,
    impact: i16,
    urgency: i16,
}

async fn check(conn: &mut PgConnection, input: &TicketTemplateInput) -> DeskResult<Checked> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| DeskError::invalid("name", "a template needs a name"))?;
    let type_key = input.type_key.clone().unwrap_or_else(|| "incident".into());
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sd_ticket_types WHERE key = $1)")
            .bind(&type_key)
            .fetch_one(&mut *conn)
            .await?;
    if !known {
        return Err(DeskError::invalid("typeKey", "there's no such ticket type"));
    }
    let level = |field: &str, v: Option<i16>| {
        let v = v.unwrap_or(2);
        if (1..=3).contains(&v) {
            Ok(v)
        } else {
            Err(DeskError::invalid(field, "1 (high) to 3 (low)"))
        }
    };
    if let Some(queue) = input.queue_id {
        let open: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sd_queues WHERE id = $1 AND archived_at IS NULL)",
        )
        .bind(queue)
        .fetch_one(&mut *conn)
        .await?;
        if !open {
            return Err(DeskError::invalid("queueId", "there's no such queue"));
        }
    }
    Ok(Checked {
        name,
        type_key,
        title: text("title", input.title.as_deref(), 300)?.unwrap_or_default(),
        description: text("description", input.description.as_deref(), 100_000)?
            .unwrap_or_default(),
        impact: level("impact", input.impact)?,
        urgency: level("urgency", input.urgency)?,
    })
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_ticket_templates_name") => {
            DeskError::invalid("name", "another template has this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &TicketTemplateInput,
) -> DeskResult<TicketTemplate> {
    let c = check(conn, input).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_ticket_templates (id, name, type_key, title, description, impact, urgency, queue_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.type_key)
    .bind(&c.title)
    .bind(&c.description)
    .bind(c.impact)
    .bind(c.urgency)
    .bind(input.queue_id)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("ticket_template.created")
            .target("ticket_template", id)
            .details(json!({ "name": c.name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &TicketTemplateInput,
) -> DeskResult<TicketTemplate> {
    let c = check(conn, input).await?;
    let rows = sqlx::query(
        "UPDATE sd_ticket_templates SET name = $3, type_key = $4, title = $5, description = $6, impact = $7, \
                urgency = $8, queue_id = $9, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&c.name)
    .bind(&c.type_key)
    .bind(&c.title)
    .bind(&c.description)
    .bind(c.impact)
    .bind(c.urgency)
    .bind(input.queue_id)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        get(conn, id).await?;
        return Err(DeskError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("ticket_template.updated")
            .target("ticket_template", id)
            .details(json!({ "name": c.name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let found = get(conn, id).await?;
    sqlx::query("UPDATE sd_ticket_templates SET archived_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("ticket_template.archived")
            .target("ticket_template", id)
            .details(json!({ "name": found.name })),
    )
    .await?;
    Ok(())
}
