//! Article categories, per organization.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{KbError, KbResult, taken, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = KbCategory)]
pub struct Category {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub description: String,
    pub position: i32,
    /// Published articles in it.
    pub articles: i64,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = KbCategoryInput)]
pub struct CategoryInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub position: Option<i32>,
}

const SELECT: &str = "SELECT c.id, c.org_id, c.name, c.description, c.position, \
    (SELECT count(*) FROM kb_articles a WHERE a.category_id = c.id AND a.published_revision IS NOT NULL AND a.status <> 'archived') AS articles \
    FROM kb_categories c";

/// The categories the transaction's scope can see, in order.
pub async fn list(conn: &mut PgConnection) -> KbResult<Vec<Category>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY c.position, lower(c.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> KbResult<Category> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE c.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(KbError::NotFound)
}

fn checked(input: &CategoryInput) -> KbResult<(String, String)> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| KbError::invalid("name", "a category needs a name"))?;
    let description = text("description", input.description.as_deref(), 500)?.unwrap_or_default();
    Ok((name, description))
}

const NAME_TAKEN: &str = "there's already a category with this name here";

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &CategoryInput,
) -> KbResult<Category> {
    let (name, description) = checked(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO kb_categories (id, org_id, name, description, position) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&name)
    .bind(&description)
    .bind(input.position.unwrap_or(0))
    .execute(&mut *conn)
    .await
    .map_err(|e| taken(e, "kb_categories_name", "name", NAME_TAKEN))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("kb.category_created")
            .org(org_id)
            .target("kb_category", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &CategoryInput,
) -> KbResult<Category> {
    let before = get(conn, id).await?;
    let (name, description) = checked(input)?;
    sqlx::query(
        "UPDATE kb_categories SET name = $2, description = $3, position = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(&description)
    .bind(input.position.unwrap_or(before.position))
    .execute(&mut *conn)
    .await
    .map_err(|e| taken(e, "kb_categories_name", "name", NAME_TAKEN))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("kb.category_updated")
            .org(before.org_id)
            .target("kb_category", id)
            .details(json!({ "name": name })),
    )
    .await?;
    get(conn, id).await
}

/// Deletes a category; its articles stay, uncategorized.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> KbResult<()> {
    let before = get(conn, id).await?;
    sqlx::query("DELETE FROM kb_categories WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("kb.category_deleted")
            .org(before.org_id)
            .target("kb_category", id)
            .details(json!({ "name": before.name })),
    )
    .await?;
    Ok(())
}
