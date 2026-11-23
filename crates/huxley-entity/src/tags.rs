//! Tags: instance-wide labels applied to entities.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{EntityError, EntityResult, entity_type, owning_org};

const COLORS: &[&str] = &["neutral", "accent", "success", "warning", "danger", "info"];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: Uuid,
    pub name: String,
    /// One of neutral, accent, success, warning, danger, info.
    pub color: String,
    pub description: String,
    /// How many records carry the tag (that the caller can see).
    pub usage: i64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagInput {
    pub name: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub description: String,
}

fn default_color() -> String {
    "neutral".into()
}

const SELECT_TAGS: &str = "SELECT t.id, t.name, t.color, t.description, \
    (SELECT count(*) FROM entity_tags e WHERE e.tag_id = t.id) AS usage FROM tags t";

fn check(input: &TagInput) -> EntityResult<(String, String, String)> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 50 || name.chars().any(char::is_control) {
        return Err(EntityError::invalid(
            "name",
            "Enter a name of 1 to 50 characters.",
        ));
    }
    if !COLORS.contains(&input.color.as_str()) {
        return Err(EntityError::invalid("color", "Choose one of the colors."));
    }
    if input.description.chars().count() > 200 {
        return Err(EntityError::invalid(
            "description",
            "Keep it under 200 characters.",
        ));
    }
    Ok((
        name.to_owned(),
        input.color.clone(),
        input.description.trim().to_owned(),
    ))
}

fn map_unique(e: sqlx::Error) -> EntityError {
    match huxley_db::violated_constraint(&e) {
        Some("tags_name_unique") => EntityError::TagNameTaken,
        _ => e.into(),
    }
}

fn db_err(e: huxley_db::DbError) -> EntityError {
    match e {
        huxley_db::DbError::Sqlx(e) => EntityError::Db(e),
        other => EntityError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

pub async fn list(conn: &mut PgConnection) -> EntityResult<Vec<Tag>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TAGS} ORDER BY lower(t.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> EntityResult<Tag> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TAGS} WHERE t.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(EntityError::NotFound)
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &TagInput,
) -> EntityResult<Tag> {
    let (name, color, description) = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO tags (id, name, color, description) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(&name)
        .bind(&color)
        .bind(&description)
        .execute(&mut *conn)
        .await
        .map_err(map_unique)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("tag.created")
            .target("tag", id)
            .details(json!({ "name": name })),
    )
    .await
    .map_err(db_err)?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &TagInput,
) -> EntityResult<Tag> {
    let before = get(conn, id).await?;
    let (name, color, description) = check(input)?;
    sqlx::query("UPDATE tags SET name = $2, color = $3, description = $4 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(&color)
        .bind(&description)
        .execute(&mut *conn)
        .await
        .map_err(map_unique)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("tag.updated")
            .target("tag", id)
            .details(json!({ "before": before.name, "after": name })),
    )
    .await
    .map_err(db_err)?;
    get(conn, id).await
}

/// Deletes a tag and removes it from every record.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> EntityResult<()> {
    let tag = get(conn, id).await?;
    sqlx::query("DELETE FROM tags WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("tag.deleted")
            .target("tag", id)
            .details(json!({ "name": tag.name })),
    )
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Tags on an entity (in the caller's RLS scope).
pub async fn of_entity(
    conn: &mut PgConnection,
    entity_type: &str,
    entity_id: Uuid,
) -> EntityResult<Vec<Tag>> {
    self::entity_type(entity_type)?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_TAGS} WHERE t.id IN (SELECT tag_id FROM entity_tags WHERE entity_type = $1 AND entity_id = $2) \
         ORDER BY lower(t.name)"
    )))
    .bind(entity_type)
    .bind(entity_id)
    .fetch_all(conn)
    .await?)
}

/// Tags an entity (idempotent). Run after checking the entity's update permission.
pub async fn apply(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: Uuid,
    tag_id: Uuid,
) -> EntityResult<()> {
    let tag = get(conn, tag_id).await?;
    let (org_id, _) = owning_org(conn, entity_type, entity_id).await?;
    let added = sqlx::query(
        "INSERT INTO entity_tags (tag_id, entity_type, entity_id, org_id, created_by) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT DO NOTHING",
    )
    .bind(tag_id)
    .bind(entity_type)
    .bind(entity_id)
    .bind(org_id)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if added > 0 {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("entity.tagged")
                .org(org_id)
                .target(entity_type, entity_id)
                .details(json!({ "tag": tag.name })),
        )
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

pub async fn remove(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: Uuid,
    tag_id: Uuid,
) -> EntityResult<()> {
    let tag = get(conn, tag_id).await?;
    let (org_id, _) = owning_org(conn, entity_type, entity_id).await?;
    let removed = sqlx::query(
        "DELETE FROM entity_tags WHERE tag_id = $1 AND entity_type = $2 AND entity_id = $3",
    )
    .bind(tag_id)
    .bind(entity_type)
    .bind(entity_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if removed > 0 {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("entity.untagged")
                .org(org_id)
                .target(entity_type, entity_id)
                .details(json!({ "tag": tag.name })),
        )
        .await
        .map_err(db_err)?;
    }
    Ok(())
}
