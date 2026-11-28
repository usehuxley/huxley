//! Project status reports (ADR-059): the facts for a period (what got done,
//! hours, milestones, health, risks) for a person or an AI to write up,
//! and the reports themselves, optionally shared in the client's portal.
//! Saving one raises `project.status_report`.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::projects::{self, Project};
use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub id: Uuid,
    pub project_id: Uuid,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    /// Markdown.
    pub body: String,
    pub health: String,
    pub client_visible: bool,
    pub author: String,
    /// Written by a workflow (usually an AI draft), not a person.
    pub automated: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewStatusReport {
    pub body: String,
    #[serde(default)]
    pub client_visible: bool,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub period_start: Option<String>,
    #[serde(default)]
    pub period_end: Option<String>,
}

const COLUMNS: &str = "id, project_id, to_char(period_start, 'YYYY-MM-DD') AS period_start, \
     to_char(period_end, 'YYYY-MM-DD') AS period_end, body, health, client_visible, author, (created_by IS NULL) AS automated, created_at";

/// A project's reports, newest first.
pub async fn list(
    conn: &mut PgConnection,
    project: Uuid,
    limit: i64,
) -> DeskResult<Vec<StatusReport>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM prj_status_reports WHERE project_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
    )))
    .bind(project)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Saves a report on a project (by `author`), raising
/// `project.status_report`.
pub async fn save(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    project: Uuid,
    input: &NewStatusReport,
    author: &str,
) -> DeskResult<StatusReport> {
    let body = input.body.trim();
    if body.is_empty() || body.chars().count() > 20_000 {
        return Err(DeskError::invalid("body", "1 to 20,000 characters"));
    }
    let date = |s: &Option<String>, f| {
        s.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| projects::parse_date(s, f))
            .transpose()
    };
    let (start, end) = (
        date(&input.period_start, "periodStart")?,
        date(&input.period_end, "periodEnd")?,
    );
    let current = projects::get(&mut *conn, project).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO prj_status_reports (id, project_id, org_id, period_start, period_end, body, health, client_visible, \
                                         author, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(project)
    .bind(current.org_id)
    .bind(start)
    .bind(end)
    .bind(body)
    .bind(&current.health.level)
    .bind(input.client_visible)
    .bind(author.trim().chars().take(200).collect::<String>())
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    projects::emit(
        &mut *conn,
        "project.status_report",
        project,
        current.org_id,
        json!({ "reportId": id, "projectName": current.name, "health": current.health.level,
                "clientVisible": input.client_visible, "author": author }),
    )
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.status_report")
            .org(current.org_id)
            .target("project", project),
    )
    .await?;
    let report: StatusReport = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM prj_status_reports WHERE id = $1"
    )))
    .bind(id)
    .fetch_one(conn)
    .await?;
    Ok(report)
}

/// Shares a report with the client in the portal, or stops sharing it.
pub async fn set_visible(
    conn: &mut PgConnection,
    project: Uuid,
    id: Uuid,
    visible: bool,
) -> DeskResult<StatusReport> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE prj_status_reports SET client_visible = $3 WHERE id = $1 AND project_id = $2 RETURNING {COLUMNS}"
    )))
    .bind(id)
    .bind(project)
    .bind(visible)
    .fetch_optional(conn)
    .await?
    .ok_or(DeskError::NotFound)
}

/// Deletes a report of a project.
pub async fn delete(conn: &mut PgConnection, project: Uuid, id: Uuid) -> DeskResult<bool> {
    Ok(
        sqlx::query("DELETE FROM prj_status_reports WHERE id = $1 AND project_id = $2")
            .bind(id)
            .bind(project)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

/// What happened on a project over a period, for writing a report.
#[derive(Debug, Clone, Serialize)]
pub struct Facts {
    pub project_id: Uuid,
    pub name: String,
    pub client: String,
    pub status: String,
    pub period_start: String,
    pub period_end: String,
    pub health: String,
    pub health_reasons: Vec<String>,
    pub budget_hours: Option<f64>,
    pub used_hours: f64,
    pub burn_percent: Option<f64>,
    pub hours_in_period: f64,
    pub tasks_done: i64,
    pub tasks_total: i64,
    /// Tasks finished in the period.
    pub completed: Vec<String>,
    /// Open tasks waiting on another.
    pub waiting: Vec<String>,
    /// Open tasks past their due date.
    pub overdue: Vec<String>,
    pub milestones_reached: Vec<String>,
    /// Unreached milestones, with due dates.
    pub milestones_upcoming: Vec<String>,
    pub scope_changes_pending: Vec<String>,
    /// The manager as a `core.notify` recipient (`user:<id>`), if any.
    pub manager_recipient: Option<String>,
}

/// The facts for a project over the `days` up to today.
pub async fn facts(conn: &mut PgConnection, project: &Project, days: i64) -> DeskResult<Facts> {
    let today = OffsetDateTime::now_utc().date();
    let since = today - Duration::days(days.clamp(1, 92));
    let completed: Vec<String> = sqlx::query_scalar(
        "SELECT t.title FROM prj_tasks k JOIN sd_tickets t ON t.id = k.ticket_id JOIN sd_statuses s ON s.id = t.status_id \
          WHERE k.project_id = $1 AND s.category IN ('resolved', 'closed') AND t.resolved_at >= $2::date \
          ORDER BY t.resolved_at",
    )
    .bind(project.id)
    .bind(since)
    .fetch_all(&mut *conn)
    .await?;
    let minutes: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(e.minutes), 0)::bigint FROM sd_time_entries e JOIN prj_tasks k ON k.ticket_id = e.ticket_id \
          WHERE k.project_id = $1 AND e.worked_on >= $2",
    )
    .bind(project.id)
    .bind(since)
    .fetch_one(&mut *conn)
    .await?;
    let reached: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM prj_milestones WHERE project_id = $1 AND reached_at >= $2::date ORDER BY reached_at",
    )
    .bind(project.id)
    .bind(since)
    .fetch_all(conn)
    .await?;
    let open = |c: &str| !matches!(c, "resolved" | "closed");
    let due_before = |d: &Option<String>, day: Date| {
        d.as_deref()
            .and_then(|d| projects::parse_date(d, "d").ok())
            .is_some_and(|d| d < day)
    };
    #[allow(clippy::cast_precision_loss)]
    let hours_in_period = minutes as f64 / 60.0;
    Ok(Facts {
        project_id: project.id,
        name: project.name.clone(),
        client: project.org_name.clone(),
        status: project.status.clone(),
        period_start: since.to_string(),
        period_end: today.to_string(),
        health: project.health.level.clone(),
        health_reasons: project.health.reasons.clone(),
        budget_hours: project.budget_hours,
        used_hours: project.used_hours,
        burn_percent: project.burn_percent,
        hours_in_period: (hours_in_period * 10.0).round() / 10.0,
        tasks_done: project.progress.done,
        tasks_total: project.progress.total,
        completed,
        waiting: project
            .tasks
            .iter()
            .filter(|t| t.waiting && open(&t.status_category))
            .map(|t| t.title.clone())
            .collect(),
        overdue: project
            .tasks
            .iter()
            .filter(|t| open(&t.status_category) && due_before(&t.due_on, today))
            .map(|t| t.title.clone())
            .collect(),
        milestones_reached: reached,
        milestones_upcoming: project
            .milestones
            .iter()
            .filter(|m| m.reached_at.is_none())
            .map(|m| match &m.due_on {
                Some(d) => format!("{} (due {d})", m.name),
                None => m.name.clone(),
            })
            .collect(),
        scope_changes_pending: project
            .scope_changes
            .iter()
            .filter(|c| c.state == "submitted")
            .map(|c| format!("#{} {}", c.number, c.title))
            .collect(),
        manager_recipient: project.manager_user_id.map(|u| format!("user:{u}")),
    })
}

/// Active projects in the transaction's scope (or one), with their facts.
pub async fn active_facts(
    conn: &mut PgConnection,
    project: Option<Uuid>,
    days: i64,
) -> DeskResult<Vec<Facts>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM prj_projects WHERE ($1::uuid IS NULL AND status = 'active') OR id = $1 ORDER BY name",
    )
    .bind(project)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for id in ids {
        let p = projects::get(&mut *conn, id).await?;
        out.push(facts(&mut *conn, &p, days).await?);
    }
    Ok(out)
}
