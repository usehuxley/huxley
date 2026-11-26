//! Scope change requests (ADR-057): more (or less) work on a project,
//! drafted by the provider and submitted for the client's approval.
//! Submitting raises `project.scope_change_submitted`; the decision (from
//! a workflow's approval, via `core.project.scope.decide`) raises
//! `project.scope_change_approved` or `_rejected`, and an approved one's
//! hours are added to the project's budget.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::projects;
use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ScopeChange {
    pub id: Uuid,
    pub project_id: Uuid,
    /// Numbered within the project.
    pub number: i32,
    pub title: String,
    pub description: String,
    /// Hours it adds to the budget (negative: removes).
    pub hours: f64,
    /// What it costs the client, if priced.
    pub amount: Option<f64>,
    /// `draft`, `submitted`, `approved`, `rejected` or `withdrawn`.
    pub state: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub decided_by: Option<String>,
    pub comment: Option<String>,
    pub version: i32,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeChangeInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hours: f64,
    #[serde(default)]
    pub amount: Option<f64>,
    /// When changing one: the version you read.
    #[serde(default)]
    pub version: Option<i32>,
}

const COLUMNS: &str = "id, project_id, number, title, description, hours::float8 AS hours, amount::float8 AS amount, \
     state, submitted_at, decided_at, decided_by, comment, version";

/// A project's scope changes, in order.
pub async fn list(conn: &mut PgConnection, project: Uuid) -> DeskResult<Vec<ScopeChange>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM prj_scope_changes WHERE project_id = $1 ORDER BY number"
    )))
    .bind(project)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<ScopeChange> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM prj_scope_changes WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(DeskError::NotFound)
}

fn check(input: &ScopeChangeInput) -> DeskResult<()> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(DeskError::invalid("title", "1 to 200 characters"));
    }
    if input.description.chars().count() > 20_000 {
        return Err(DeskError::invalid("description", "up to 20,000 characters"));
    }
    if !(-100_000.0..=100_000.0).contains(&input.hours) {
        return Err(DeskError::invalid("hours", "-100,000 to 100,000"));
    }
    if input
        .amount
        .is_some_and(|a| !(-100_000_000.0..=100_000_000.0).contains(&a))
    {
        return Err(DeskError::invalid("amount", "up to 100,000,000"));
    }
    Ok(())
}

/// Drafts a scope change on a project.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    project: Uuid,
    input: &ScopeChangeInput,
) -> DeskResult<ScopeChange> {
    check(input)?;
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM prj_projects WHERE id = $1")
        .bind(project)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(DeskError::NotFound)?;
    // Numbering: serialize on the project's row.
    sqlx::query("SELECT 1 FROM prj_projects WHERE id = $1 FOR UPDATE")
        .bind(project)
        .execute(&mut *conn)
        .await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO prj_scope_changes (id, project_id, org_id, number, title, description, hours, amount, created_by) \
         VALUES ($1, $2, $3, (SELECT coalesce(max(number), 0) + 1 FROM prj_scope_changes WHERE project_id = $2), \
                 $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(project)
    .bind(org)
    .bind(input.title.trim())
    .bind(input.description.trim())
    .bind(input.hours)
    .bind(input.amount)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.scope_change.created")
            .org(org)
            .target("project", project),
    )
    .await?;
    get(conn, id).await
}

/// Changes a draft.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    input: &ScopeChangeInput,
) -> DeskResult<ScopeChange> {
    check(input)?;
    let current = get(&mut *conn, id).await?;
    if current.state != "draft" {
        return Err(DeskError::invalid("state", "only a draft can be changed"));
    }
    let rows = sqlx::query(
        "UPDATE prj_scope_changes SET title = $2, description = $3, hours = $4, amount = $5, version = version + 1 \
          WHERE id = $1 AND version = $6",
    )
    .bind(id)
    .bind(input.title.trim())
    .bind(input.description.trim())
    .bind(input.hours)
    .bind(input.amount)
    .bind(input.version.unwrap_or(current.version))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    get(conn, id).await
}

/// The project a scope change belongs to, and its company.
async fn owner(
    conn: &mut PgConnection,
    id: Uuid,
) -> DeskResult<(Uuid, Uuid, String, Option<String>)> {
    sqlx::query_as(
        "SELECT p.id, p.org_id, p.name, a.currency FROM prj_scope_changes c JOIN prj_projects p ON p.id = c.project_id \
           LEFT JOIN contract_agreements a ON a.id = p.agreement_id WHERE c.id = $1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(DeskError::NotFound)
}

/// Submits a draft for the client's approval: raises
/// `project.scope_change_submitted`.
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> DeskResult<ScopeChange> {
    let current = get(&mut *conn, id).await?;
    if current.state != "draft" {
        return Err(DeskError::invalid("state", "only a draft can be submitted"));
    }
    let (project, org, name, currency) = owner(&mut *conn, id).await?;
    sqlx::query(
        "UPDATE prj_scope_changes SET state = 'submitted', submitted_at = now(), version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    projects::emit(
        &mut *conn,
        "project.scope_change_submitted",
        project,
        org,
        json!({
            "scopeChangeId": id, "projectName": name, "number": current.number, "title": current.title,
            "description": current.description, "hours": current.hours, "amount": current.amount,
            "currency": currency,
        }),
    )
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.scope_change.submitted")
            .org(org)
            .target("project", project),
    )
    .await?;
    get(conn, id).await
}

/// Withdraws a draft or submitted change.
pub async fn withdraw(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> DeskResult<ScopeChange> {
    let current = get(&mut *conn, id).await?;
    if !matches!(current.state.as_str(), "draft" | "submitted") {
        return Err(DeskError::invalid("state", "it's already decided"));
    }
    let (project, org, _, _) = owner(&mut *conn, id).await?;
    sqlx::query(
        "UPDATE prj_scope_changes SET state = 'withdrawn', version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.scope_change.withdrawn")
            .org(org)
            .target("project", project),
    )
    .await?;
    get(conn, id).await
}

/// Records the decision on a submitted change. Approved: its hours are
/// added to the budget. Raises `project.scope_change_approved` or
/// `project.scope_change_rejected`.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    approved: bool,
    by: &str,
    comment: Option<&str>,
) -> DeskResult<ScopeChange> {
    let current = get(&mut *conn, id).await?;
    if current.state != "submitted" {
        return Err(DeskError::invalid(
            "state",
            "it isn't waiting for a decision",
        ));
    }
    let (project, org, name, _) = owner(&mut *conn, id).await?;
    let comment = comment.map(str::trim).filter(|c| !c.is_empty());
    sqlx::query(
        "UPDATE prj_scope_changes SET state = $2, decided_at = now(), decided_by = $3, comment = $4, \
                version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .bind(if approved { "approved" } else { "rejected" })
    .bind(by)
    .bind(comment)
    .execute(&mut *conn)
    .await?;
    if approved && current.hours != 0.0 {
        sqlx::query(
            "UPDATE prj_projects SET budget_hours = greatest(coalesce(budget_hours, 0) + $2, 0), \
                    version = version + 1, updated_at = now() WHERE id = $1",
        )
        .bind(project)
        .bind(current.hours)
        .execute(&mut *conn)
        .await?;
        projects::check_budget(&mut *conn, project).await?;
    }
    projects::emit(
        &mut *conn,
        if approved {
            "project.scope_change_approved"
        } else {
            "project.scope_change_rejected"
        },
        project,
        org,
        json!({
            "scopeChangeId": id, "projectName": name, "number": current.number, "title": current.title,
            "hours": current.hours, "amount": current.amount, "by": by, "comment": comment,
        }),
    )
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.scope_change.decided")
            .org(org)
            .target("project", project)
            .details(json!({ "number": current.number, "approved": approved, "by": by })),
    )
    .await?;
    get(conn, id).await
}
