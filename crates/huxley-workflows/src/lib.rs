//! Workflow definitions (design doc §6.1): a draft people edit, and
//! immutable published versions that runs pin to.
//!
//! Drafts may be saved in any state, so the canvas can save work in
//! progress; every read reports the draft's problems as diagnostics.
//! Publishing requires none: structure (`huxley-hwf`) and expressions
//! (`huxley-expr`) both have to check out.

pub mod triggers;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_hwf::Diagnostic;
use serde::Serialize;
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("workflow not found")]
    NotFound,
    #[error("version not found")]
    VersionNotFound,
    #[error("another workflow in this organization already has the key {0:?}")]
    KeyTaken(String),
    #[error("{0}")]
    InvalidDocument(String),
    /// Publishing (or running) needs a valid document.
    #[error("the workflow has problems")]
    Invalid(Vec<Diagnostic>),
    #[error("the draft changed since revision {0}; reload it and try again")]
    RevisionConflict(i32),
    #[error("the workflow is archived")]
    Archived,
    #[error("the inputs don't match the workflow's input schema")]
    InvalidInputs(Vec<String>),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type WorkflowResult<T> = Result<T, WorkflowError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "workflow_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: WorkflowStatus,
    /// The latest published version, if any.
    pub current_version: Option<i32>,
    /// Increases with every draft save.
    pub revision: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A workflow with its draft and the draft's problems.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDetail {
    #[serde(flatten)]
    pub summary: WorkflowSummary,
    /// The HWF document being edited.
    #[schema(value_type = Object)]
    pub draft: Json,
    /// Problems that would stop the draft being published. Empty when it's ready.
    #[schema(value_type = Vec<DiagnosticView>)]
    pub diagnostics: Vec<Diagnostic>,
}

/// A problem with a workflow document, and where it is.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DiagnosticView {
    /// e.g. `spec.steps[2].then[0].id`
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub version: i32,
    pub notes: Option<String>,
    pub published_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    #[serde(flatten)]
    pub summary: VersionSummary,
    #[schema(value_type = Object)]
    pub document: Json,
}

/// Every problem with a document: whether it reads as HWF at all, its
/// structure, and its expressions.
pub fn diagnose(expr: &huxley_expr::Engine, document: &Json) -> Vec<Diagnostic> {
    match huxley_hwf::from_value(document.clone()) {
        Ok(workflow) => huxley_expr::check_workflow(expr, &workflow),
        Err(huxley_hwf::HwfError::Invalid(diagnostics)) => diagnostics,
        Err(other) => vec![Diagnostic {
            path: String::new(),
            message: other.to_string(),
        }],
    }
}

/// The key and name a document declares: needed even for a rough draft.
fn identity(document: &Json) -> WorkflowResult<(String, String, Option<String>)> {
    let meta = &document["metadata"];
    let key = meta["key"].as_str().unwrap_or_default().to_owned();
    if !huxley_hwf::is_valid_key(&key) {
        return Err(WorkflowError::InvalidDocument(
            "metadata.key: use lowercase letters, digits and hyphens, starting with a letter"
                .into(),
        ));
    }
    let name = meta["name"].as_str().unwrap_or_default().trim().to_owned();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(WorkflowError::InvalidDocument(
            "metadata.name: 1 to 200 characters".into(),
        ));
    }
    let description = meta["description"].as_str().map(str::to_owned);
    Ok((key, name, description))
}

fn detail(expr: &huxley_expr::Engine, summary: WorkflowSummary, draft: Json) -> WorkflowDetail {
    let diagnostics = diagnose(expr, &draft);
    WorkflowDetail {
        summary,
        draft,
        diagnostics,
    }
}

/// Workflows visible in the caller's scope, by name.
pub async fn list(conn: &mut PgConnection) -> WorkflowResult<Vec<WorkflowSummary>> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, key, name, description, status, current_version, revision, \
                created_at, updated_at FROM workflows ORDER BY lower(name), id",
    )
    .fetch_all(conn)
    .await?)
}

pub async fn summary(conn: &mut PgConnection, id: Uuid) -> WorkflowResult<WorkflowSummary> {
    sqlx::query_as(
        "SELECT id, org_id, key, name, description, status, current_version, revision, \
                created_at, updated_at FROM workflows WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(WorkflowError::NotFound)
}

pub async fn get(
    conn: &mut PgConnection,
    expr: &huxley_expr::Engine,
    id: Uuid,
) -> WorkflowResult<WorkflowDetail> {
    let summary = summary(&mut *conn, id).await?;
    let (draft,): (Json,) = sqlx::query_as("SELECT draft FROM workflows WHERE id = $1")
        .bind(id)
        .fetch_one(conn)
        .await?;
    Ok(detail(expr, summary, draft))
}

/// Creates a workflow from a (possibly incomplete) draft.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    expr: &huxley_expr::Engine,
    org_id: Uuid,
    document: Json,
) -> WorkflowResult<WorkflowDetail> {
    let (key, name, description) = identity(&document)?;
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO workflows (id, org_id, key, name, description, draft, created_by, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7) ON CONFLICT (org_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(org_id)
    .bind(&key)
    .bind(&name)
    .bind(&description)
    .bind(&document)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(WorkflowError::KeyTaken(key));
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("workflow.created")
            .org(org_id)
            .target("workflow", id)
            .details(json!({ "key": key, "name": name })),
    )
    .await
    .map_err(db)?;
    get(conn, expr, id).await
}

fn db(e: huxley_db::DbError) -> WorkflowError {
    match e {
        huxley_db::DbError::Sqlx(e) => WorkflowError::Db(e),
        other => WorkflowError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

/// Replaces the draft, if it is still at `revision`. The key can't change.
pub async fn save_draft(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    expr: &huxley_expr::Engine,
    id: Uuid,
    revision: i32,
    document: Json,
) -> WorkflowResult<WorkflowDetail> {
    let current = summary(&mut *conn, id).await?;
    if current.status == WorkflowStatus::Archived {
        return Err(WorkflowError::Archived);
    }
    let (key, name, description) = identity(&document)?;
    if key != current.key {
        return Err(WorkflowError::InvalidDocument(format!(
            "metadata.key can't change (it is {:?})",
            current.key
        )));
    }
    let updated = sqlx::query(
        "UPDATE workflows SET draft = $3, name = $4, description = $5, updated_by = $6, \
                updated_at = now(), revision = revision + 1 \
          WHERE id = $1 AND revision = $2",
    )
    .bind(id)
    .bind(revision)
    .bind(&document)
    .bind(&name)
    .bind(&description)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(WorkflowError::RevisionConflict(revision));
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("workflow.draft_saved")
            .org(current.org_id)
            .target("workflow", id)
            .details(json!({ "revision": revision + 1 })),
    )
    .await
    .map_err(db)?;
    get(conn, expr, id).await
}

/// Publishes the draft as the next version. It must have no problems.
pub async fn publish(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    expr: &huxley_expr::Engine,
    id: Uuid,
    notes: Option<&str>,
) -> WorkflowResult<Version> {
    let (org_id, status, draft, current): (Uuid, WorkflowStatus, Json, Option<i32>) =
        sqlx::query_as(
            "SELECT org_id, status, draft, current_version FROM workflows WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(WorkflowError::NotFound)?;
    if status == WorkflowStatus::Archived {
        return Err(WorkflowError::Archived);
    }
    let diagnostics = diagnose(expr, &draft);
    if !diagnostics.is_empty() {
        return Err(WorkflowError::Invalid(diagnostics));
    }
    let version = current.unwrap_or(0) + 1;
    let notes = notes.map(str::trim).filter(|n| !n.is_empty());
    sqlx::query(
        "INSERT INTO workflow_versions (workflow_id, version, org_id, document, notes, published_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(version)
    .bind(org_id)
    .bind(&draft)
    .bind(notes)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE workflows SET current_version = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(version)
        .execute(&mut *conn)
        .await?;
    triggers::sync(&mut *conn, id, org_id, &draft).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("workflow.published")
            .org(org_id)
            .target("workflow", id)
            .details(json!({ "version": version })),
    )
    .await
    .map_err(db)?;
    self::version(conn, id, version).await
}

pub async fn versions(conn: &mut PgConnection, id: Uuid) -> WorkflowResult<Vec<VersionSummary>> {
    summary(&mut *conn, id).await?;
    Ok(sqlx::query_as(
        "SELECT version, notes, published_by, published_at FROM workflow_versions \
          WHERE workflow_id = $1 ORDER BY version DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

pub async fn version(conn: &mut PgConnection, id: Uuid, version: i32) -> WorkflowResult<Version> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        summary: VersionSummary,
        document: Json,
    }
    let row: Row = sqlx::query_as(
        "SELECT version, notes, published_by, published_at, document FROM workflow_versions \
          WHERE workflow_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(conn)
    .await?
    .ok_or(WorkflowError::VersionNotFound)?;
    Ok(Version {
        summary: row.summary,
        document: row.document,
    })
}

/// Archives (or restores) a workflow. Archived workflows can't be edited,
/// published or run; their history stays.
pub async fn set_archived(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    archived: bool,
) -> WorkflowResult<WorkflowSummary> {
    let status = if archived {
        WorkflowStatus::Archived
    } else {
        WorkflowStatus::Active
    };
    let org: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE workflows SET status = $2, updated_at = now() WHERE id = $1 RETURNING org_id",
    )
    .bind(id)
    .bind(status)
    .fetch_optional(&mut *conn)
    .await?;
    let (org_id,) = org.ok_or(WorkflowError::NotFound)?;
    // Archived workflows don't trigger; restoring brings the published
    // version's triggers back.
    if archived {
        triggers::clear(&mut *conn, id).await?;
    } else {
        let published: Option<(Json,)> = sqlx::query_as(
            "SELECT v.document FROM workflows w \
               JOIN workflow_versions v ON v.workflow_id = w.id AND v.version = w.current_version \
              WHERE w.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((document,)) = published {
            triggers::sync(&mut *conn, id, org_id, &document).await?;
        }
    }
    let action = if archived {
        "workflow.archived"
    } else {
        "workflow.restored"
    };
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action).org(org_id).target("workflow", id),
    )
    .await
    .map_err(db)?;
    summary(conn, id).await
}

/// Checks run inputs against the document's input schema (if it has one).
pub fn check_inputs(document: &Json, inputs: &Json) -> WorkflowResult<()> {
    let Some(schema) = document["spec"].get("inputs").filter(|s| !s.is_null()) else {
        return Ok(());
    };
    let validator = jsonschema::validator_for(schema).map_err(|e| {
        WorkflowError::InvalidDocument(format!("spec.inputs isn't a valid JSON Schema: {e}"))
    })?;
    let problems: Vec<String> = validator
        .iter_errors(inputs)
        .map(|e| {
            let at = e.instance_path().to_string();
            if at.is_empty() {
                e.to_string()
            } else {
                format!("{at}: {e}")
            }
        })
        .take(20)
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(WorkflowError::InvalidInputs(problems))
    }
}
