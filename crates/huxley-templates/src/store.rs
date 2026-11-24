//! Your organization's templates, saved from workflows. A template at an
//! organization is offered to it and to every organization below it.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{FormPart, Template, TemplateInfo};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no such template")]
    NotFound,
    #[error("{1}")]
    Invalid(String, String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

fn db(e: huxley_db::DbError) -> StoreError {
    match e {
        huxley_db::DbError::Sqlx(e) => StoreError::Db(e),
        other => StoreError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    org_id: Uuid,
    name: String,
    description: String,
    category: String,
    workflow: Json,
    form: Option<Json>,
}

impl Row {
    fn into_template(self) -> Template {
        let form: Option<FormPart> = self.form.and_then(|f| serde_json::from_value(f).ok());
        Template {
            template: TemplateInfo {
                key: self.id.to_string(),
                name: self.name,
                description: self.description,
                category: self.category,
                bindings: Vec::new(),
                has_form: form.is_some(),
                org_id: Some(self.org_id),
            },
            form,
            workflow: self.workflow,
        }
    }
}

const COLUMNS: &str = "t.id, t.org_id, t.name, t.description, t.category, t.workflow, t.form";

/// Saves a template at `org_id`.
pub async fn save(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    template: &Template,
) -> Result<TemplateInfo, StoreError> {
    let info = &template.template;
    let name = info.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(StoreError::Invalid(
            "name".into(),
            "1 to 200 characters".into(),
        ));
    }
    let category = info.category.trim();
    let category = if category.is_empty() {
        "Your templates"
    } else {
        category
    };
    if category.chars().count() > 100 || info.description.chars().count() > 2000 {
        return Err(StoreError::Invalid("category".into(), "too long".into()));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO workflow_templates (id, org_id, name, description, category, workflow, form, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org_id)
    .bind(name)
    .bind(info.description.trim())
    .bind(category)
    .bind(&template.workflow)
    .bind(template.form.as_ref().map(|f| serde_json::to_value(f).unwrap_or_default()))
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("template.saved")
            .org(org_id)
            .target("template", id)
            .details(json!({ "name": name })),
    )
    .await
    .map_err(db)?;
    get(conn, id).await.map(|t| t.template)
}

/// One template (as the system: the caller checks access).
pub async fn get(conn: &mut PgConnection, id: Uuid) -> Result<Template, StoreError> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM workflow_templates t WHERE t.id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(StoreError::NotFound)?;
    Ok(row.into_template())
}

/// Templates offered to `org_id`: its own and those of organizations above
/// it, nearest first.
pub async fn offered(conn: &mut PgConnection, org_id: Uuid) -> Result<Vec<Template>, StoreError> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM workflow_templates t JOIN organizations o ON o.id = t.org_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path) DESC, t.name"
    )))
    .bind(org_id)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(Row::into_template).collect())
}

pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> Result<(), StoreError> {
    let template = get(&mut *conn, id).await?;
    sqlx::query("DELETE FROM workflow_templates WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("template.deleted")
            .org(template.template.org_id.unwrap_or_default())
            .target("template", id),
    )
    .await
    .map_err(db)?;
    Ok(())
}
