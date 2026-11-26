//! Folders to organize workflows: per organization, nestable. A workflow is
//! in one folder or none (unfiled).
//!
//! An organization's **System folder** holds the workflows installed from
//! Huxley's built-in packs (`workflows.system`). It's made when the first
//! one is installed, can't be renamed, moved or deleted, holds no other
//! workflows or folders, and its workflows can't leave it.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkflowError, WorkflowResult};

pub const SYSTEM_NAME: &str = "System";

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkflowFolder)]
pub struct Folder {
    pub id: Uuid,
    pub org_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    /// The System folder: built-in workflows, fixed in place.
    pub system: bool,
    /// Workflows directly in it.
    pub workflows: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A folder to make, or a folder's new name and place.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = WorkflowFolderInput)]
pub struct FolderInput {
    pub name: String,
    /// The folder it's in (none: the top level).
    #[serde(default)]
    pub parent_id: Option<Uuid>,
}

const SELECT: &str = "SELECT f.id, f.org_id, f.parent_id, f.name, f.system, \
        (SELECT count(*) FROM workflows w WHERE w.folder_id = f.id) AS workflows, f.created_at \
   FROM workflow_folders f";

fn rule(message: &str) -> WorkflowError {
    WorkflowError::FolderRule(message.to_owned())
}

/// Every folder visible here (RLS), by organization and name.
pub async fn list(conn: &mut PgConnection) -> WorkflowResult<Vec<Folder>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY f.org_id, f.system DESC, lower(f.name), f.id"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> WorkflowResult<Folder> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE f.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(WorkflowError::FolderNotFound)
}

fn clean_name(name: &str) -> WorkflowResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(WorkflowError::InvalidFolder(
            "a folder's name is 1 to 100 characters".into(),
        ));
    }
    Ok(name.to_owned())
}

fn taken(name: &str) -> impl FnOnce(sqlx::Error) -> WorkflowError + '_ {
    move |e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("workflow_folders_name") => {
            WorkflowError::FolderTaken(name.to_owned())
        }
        _ => WorkflowError::Db(e),
    }
}

/// `parent`, checked as a place for a folder or workflow of `org_id`.
async fn place(
    conn: &mut PgConnection,
    org_id: Uuid,
    parent: Option<Uuid>,
) -> WorkflowResult<Option<Folder>> {
    let Some(id) = parent else { return Ok(None) };
    let folder = get(conn, id).await?;
    if folder.org_id != org_id {
        return Err(rule(
            "a folder can only hold its own organization's folders and workflows",
        ));
    }
    Ok(Some(folder))
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &FolderInput,
) -> WorkflowResult<Folder> {
    let name = clean_name(&input.name)?;
    if place(&mut *conn, org_id, input.parent_id)
        .await?
        .is_some_and(|p| p.system)
    {
        return Err(rule("the System folder holds only built-in workflows"));
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO workflow_folders (id, org_id, parent_id, name, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.parent_id)
    .bind(&name)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await
    .map_err(taken(&name))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("workflow_folder.created")
            .org(org_id)
            .target("workflow_folder", id)
            .details(json!({ "name": name, "parentId": input.parent_id })),
    )
    .await
    .map_err(crate::db)?;
    get(conn, id).await
}

/// Renames or moves a folder (not the System folder, and not into itself
/// or a folder inside it).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &FolderInput,
) -> WorkflowResult<Folder> {
    let current = get(&mut *conn, id).await?;
    if current.system {
        return Err(rule("the System folder can't be renamed or moved"));
    }
    let name = clean_name(&input.name)?;
    if let Some(parent) = place(&mut *conn, current.org_id, input.parent_id).await? {
        if parent.system {
            return Err(rule("the System folder holds only built-in workflows"));
        }
        let loops: bool = sqlx::query_scalar(
            "WITH RECURSIVE up AS ( \
                 SELECT id, parent_id FROM workflow_folders WHERE id = $1 \
                 UNION ALL \
                 SELECT f.id, f.parent_id FROM workflow_folders f JOIN up ON f.id = up.parent_id) \
             SELECT EXISTS (SELECT 1 FROM up WHERE id = $2)",
        )
        .bind(parent.id)
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
        if loops {
            return Err(rule("a folder can't go inside itself"));
        }
    }
    sqlx::query("UPDATE workflow_folders SET name = $2, parent_id = $3 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(input.parent_id)
        .execute(&mut *conn)
        .await
        .map_err(taken(&name))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("workflow_folder.updated")
            .org(current.org_id)
            .target("workflow_folder", id)
            .details(json!({
                "name": name, "previousName": current.name,
                "parentId": input.parent_id, "previousParentId": current.parent_id,
            })),
    )
    .await
    .map_err(crate::db)?;
    get(conn, id).await
}

/// Deletes a folder (not the System folder). What was in it moves up to
/// its parent; no workflow is deleted.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> WorkflowResult<()> {
    let current = get(&mut *conn, id).await?;
    if current.system {
        return Err(rule("the System folder can't be deleted"));
    }
    // A subfolder whose name its new siblings already have gets a suffix.
    let children: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM workflow_folders WHERE parent_id = $1 ORDER BY name")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    for (child, name) in children {
        let mut candidate = name.clone();
        for n in 2.. {
            let clash: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM workflow_folders WHERE org_id = $1 \
                    AND parent_id IS NOT DISTINCT FROM $2 AND lower(name) = lower($3) AND id <> $4)",
            )
            .bind(current.org_id)
            .bind(current.parent_id)
            .bind(&candidate)
            .bind(child)
            .fetch_one(&mut *conn)
            .await?;
            if !clash {
                break;
            }
            candidate = format!("{name} ({n})");
        }
        sqlx::query("UPDATE workflow_folders SET parent_id = $2, name = $3 WHERE id = $1")
            .bind(child)
            .bind(current.parent_id)
            .bind(&candidate)
            .execute(&mut *conn)
            .await?;
    }
    let moved = sqlx::query("UPDATE workflows SET folder_id = $2 WHERE folder_id = $1")
        .bind(id)
        .bind(current.parent_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM workflow_folders WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("workflow_folder.deleted")
            .org(current.org_id)
            .target("workflow_folder", id)
            .details(json!({ "name": current.name, "workflowsMoved": moved })),
    )
    .await
    .map_err(crate::db)?;
    Ok(())
}

/// Moves a workflow into `folder` (none: unfiled). Built-in workflows stay
/// in the System folder, which holds nothing else.
pub async fn move_workflow(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    workflow_id: Uuid,
    folder: Option<Uuid>,
) -> WorkflowResult<()> {
    let workflow = crate::summary(&mut *conn, workflow_id).await?;
    if workflow.folder_id == folder {
        return Ok(());
    }
    let target = place(&mut *conn, workflow.org_id, folder).await?;
    if workflow.system {
        return Err(rule("built-in workflows stay in the System folder"));
    }
    if target.as_ref().is_some_and(|f| f.system) {
        return Err(rule("the System folder holds only built-in workflows"));
    }
    sqlx::query("UPDATE workflows SET folder_id = $2 WHERE id = $1")
        .bind(workflow_id)
        .bind(folder)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("workflow.moved")
            .org(workflow.org_id)
            .target("workflow", workflow_id)
            .details(json!({ "folderId": folder, "previousFolderId": workflow.folder_id })),
    )
    .await
    .map_err(crate::db)?;
    Ok(())
}

/// The organization's System folder, made if it hasn't one.
pub async fn system_folder(conn: &mut PgConnection, org_id: Uuid) -> WorkflowResult<Uuid> {
    sqlx::query(
        "INSERT INTO workflow_folders (id, org_id, name, system) VALUES ($1, $2, $3, true) \
         ON CONFLICT (org_id) WHERE system DO NOTHING",
    )
    .bind(huxley_core::new_id())
    .bind(org_id)
    .bind(SYSTEM_NAME)
    .execute(&mut *conn)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM workflow_folders WHERE org_id = $1 AND system")
            .bind(org_id)
            .fetch_one(conn)
            .await?,
    )
}

/// Files a workflow a pack installed: a built-in pack's in the System
/// folder (for good); another pack's, when it's new, in a top-level folder
/// named after the pack, made if needed.
pub async fn file_installed(
    conn: &mut PgConnection,
    workflow_id: Uuid,
    org_id: Uuid,
    built_in: bool,
    pack_name: &str,
    new: bool,
) -> WorkflowResult<()> {
    let folder = if built_in {
        system_folder(&mut *conn, org_id).await?
    } else if new {
        let name = clean_name(pack_name).unwrap_or_else(|_| {
            pack_name
                .chars()
                .take(100)
                .collect::<String>()
                .trim()
                .to_owned()
        });
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM workflow_folders \
              WHERE org_id = $1 AND parent_id IS NULL AND NOT system AND lower(name) = lower($2)",
        )
        .bind(org_id)
        .bind(&name)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(id) = existing {
            id
        } else {
            let id = huxley_core::new_id();
            sqlx::query("INSERT INTO workflow_folders (id, org_id, name) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(org_id)
                .bind(&name)
                .execute(&mut *conn)
                .await?;
            id
        }
    } else {
        return Ok(());
    };
    sqlx::query("UPDATE workflows SET folder_id = $2, system = $3 WHERE id = $1")
        .bind(workflow_id)
        .bind(folder)
        .bind(built_in)
        .execute(conn)
        .await?;
    Ok(())
}
