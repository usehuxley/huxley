//! Projects (ADR-056): a client's project with phases, milestones and
//! tasks (tickets of the Project task type), created blank or from a
//! template; budget burn from time on its tasks, with
//! `project.budget_threshold` raised once per threshold; health from
//! milestones and burn; what the client's portal shows.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::project_templates;
use crate::tickets::{self, NewTicket};
use crate::{DeskError, DeskResult};

pub const STATUSES: &[&str] = &["planned", "active", "on_hold", "completed", "cancelled"];
pub const BILLING: &[&str] = &[
    "fixed_fee",
    "milestone",
    "time_materials",
    "not_to_exceed",
    "agreement",
];

/// `YYYY-MM-DD`.
pub fn parse_date(s: &str, field: &str) -> DeskResult<Date> {
    let bad = || DeskError::invalid(field, "a date: YYYY-MM-DD");
    let mut parts = s.trim().splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let (y, m, d): (i32, u8, u8) = (
        y.parse().map_err(|_| bad())?,
        m.parse().map_err(|_| bad())?,
        d.parse().map_err(|_| bad())?,
    );
    Date::from_calendar_date(y, time::Month::try_from(m).map_err(|_| bad())?, d).map_err(|_| bad())
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewProject {
    /// The client company it's for.
    pub org_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub manager_user_id: Option<Uuid>,
    /// `YYYY-MM-DD`.
    pub start_on: String,
    #[serde(default)]
    pub end_on: Option<String>,
    /// Default: the template's phase budgets added up.
    #[serde(default)]
    pub budget_hours: Option<f64>,
    #[serde(default)]
    pub billing_method: Option<String>,
    #[serde(default)]
    pub template_id: Option<Uuid>,
    /// A template task's key → who's assigned it.
    #[serde(default)]
    pub assignees: BTreeMap<String, Uuid>,
    #[serde(default)]
    pub thresholds: Option<Vec<i16>>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectChanges {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub manager_user_id: Option<Uuid>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub end_on: Option<String>,
    #[serde(default)]
    pub budget_hours: Option<f64>,
    #[serde(default)]
    pub billing_method: Option<String>,
    #[serde(default)]
    pub thresholds: Option<Vec<i16>>,
    #[serde(default)]
    pub portal_visible: Option<bool>,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `on_track`, `at_risk` or `off_track`.
    pub level: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPhase {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
    pub budget_hours: Option<f64>,
    pub used_hours: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Milestone {
    pub id: Uuid,
    pub name: String,
    pub due_on: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reached_at: Option<OffsetDateTime>,
    pub client_visible: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTask {
    pub ticket_id: Uuid,
    pub reference: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub assignee_user_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub phase_id: Option<Uuid>,
    pub estimate_hours: Option<f64>,
    pub used_hours: f64,
    pub depends_on: Vec<Uuid>,
    /// A task it depends on isn't done yet.
    pub waiting: bool,
    pub client_visible: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: Uuid,
    org_id: Uuid,
    org_name: String,
    name: String,
    description: String,
    manager_user_id: Option<Uuid>,
    manager_name: Option<String>,
    status: String,
    start_on: String,
    end_on: Option<String>,
    budget_hours: Option<f64>,
    used_hours: f64,
    billing_method: String,
    thresholds: Vec<i16>,
    thresholds_fired: Vec<i16>,
    portal_visible: bool,
    template_id: Option<Uuid>,
    version: i32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub name: String,
    pub description: String,
    pub manager_user_id: Option<Uuid>,
    pub manager_name: Option<String>,
    pub status: String,
    pub start_on: String,
    pub end_on: Option<String>,
    pub budget_hours: Option<f64>,
    /// Hours logged on its tasks.
    pub used_hours: f64,
    /// Used against budget, in percent.
    pub burn_percent: Option<f64>,
    pub billing_method: String,
    pub thresholds: Vec<i16>,
    pub thresholds_fired: Vec<i16>,
    pub portal_visible: bool,
    pub template_id: Option<Uuid>,
    pub health: Health,
    pub progress: Progress,
    pub phases: Vec<ProjectPhase>,
    pub milestones: Vec<Milestone>,
    pub tasks: Vec<ProjectTask>,
    pub version: i32,
}

const SELECT: &str = "SELECT p.id, p.org_id, o.name AS org_name, p.name, p.description, p.manager_user_id, \
     u.display_name AS manager_name, p.status, to_char(p.start_on, 'YYYY-MM-DD') AS start_on, \
     to_char(p.end_on, 'YYYY-MM-DD') AS end_on, p.budget_hours::float8 AS budget_hours, \
     coalesce((SELECT sum(e.minutes) FROM sd_time_entries e JOIN prj_tasks k ON k.ticket_id = e.ticket_id \
               WHERE k.project_id = p.id), 0)::float8 / 60 AS used_hours, \
     p.billing_method, p.thresholds, p.thresholds_fired, p.portal_visible, p.template_id, p.version \
     FROM prj_projects p JOIN organizations o ON o.id = p.org_id LEFT JOIN users u ON u.id = p.manager_user_id";

fn burn(budget: Option<f64>, used: f64) -> Option<f64> {
    budget
        .filter(|b| *b > 0.0)
        .map(|b| (used / b * 1000.0).round() / 10.0)
}

/// On track, at risk or off track, and why.
fn health(row: &Row, milestones: &[Milestone], progress: &Progress, today: Date) -> Health {
    let mut reasons = Vec::new();
    let overdue: Vec<&str> = milestones
        .iter()
        .filter(|m| {
            m.reached_at.is_none()
                && m.due_on
                    .as_deref()
                    .and_then(|d| parse_date(d, "dueOn").ok())
                    .is_some_and(|d| d < today)
        })
        .map(|m| m.name.as_str())
        .collect();
    if !overdue.is_empty() {
        reasons.push(format!(
            "Overdue milestone{}: {}",
            if overdue.len() == 1 { "" } else { "s" },
            overdue.join(", ")
        ));
    }
    let burn = burn(row.budget_hours, row.used_hours);
    if burn.is_some_and(|b| b > 100.0) {
        reasons.push(format!(
            "Over budget: {:.0}% used",
            burn.unwrap_or_default()
        ));
    }
    let off = !reasons.is_empty();
    // Spending ahead of progress.
    #[allow(clippy::cast_precision_loss)]
    let done = if progress.total > 0 {
        progress.done as f64 / progress.total as f64 * 100.0
    } else {
        0.0
    };
    if let Some(b) = burn.filter(|b| *b <= 100.0 && *b > done + 25.0) {
        reasons.push(format!(
            "{b:.0}% of the budget used with {done:.0}% of tasks done"
        ));
    }
    let level = if off {
        "off_track"
    } else if reasons.is_empty() {
        "on_track"
    } else {
        "at_risk"
    };
    Health {
        level: level.into(),
        reasons,
    }
}

async fn details(
    conn: &mut PgConnection,
    id: Uuid,
) -> DeskResult<(Vec<ProjectPhase>, Vec<Milestone>, Vec<ProjectTask>)> {
    let phases: Vec<ProjectPhase> = sqlx::query_as(
        "SELECT ph.id, ph.name, ph.position, ph.budget_hours::float8 AS budget_hours, \
                coalesce((SELECT sum(e.minutes) FROM sd_time_entries e JOIN prj_tasks k ON k.ticket_id = e.ticket_id \
                          WHERE k.phase_id = ph.id), 0)::float8 / 60 AS used_hours \
           FROM prj_phases ph WHERE ph.project_id = $1 ORDER BY ph.position",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let milestones: Vec<Milestone> = sqlx::query_as(
        "SELECT id, name, to_char(due_on, 'YYYY-MM-DD') AS due_on, reached_at, client_visible \
           FROM prj_milestones WHERE project_id = $1 ORDER BY position, due_on",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    let tasks: Vec<ProjectTask> = sqlx::query_as(
        "SELECT k.ticket_id, t.number_prefix || '-' || t.number AS reference, t.title, s.name AS status_name, \
                s.category AS status_category, t.assignee_user_id, u.display_name AS assignee_name, k.phase_id, \
                k.estimate_hours::float8 AS estimate_hours, \
                coalesce((SELECT sum(e.minutes) FROM sd_time_entries e WHERE e.ticket_id = k.ticket_id), 0)::float8 / 60 AS used_hours, \
                k.depends_on, \
                EXISTS (SELECT 1 FROM sd_tickets d JOIN sd_statuses ds ON ds.id = d.status_id \
                         WHERE d.id = ANY(k.depends_on) AND ds.category NOT IN ('resolved', 'closed')) AS waiting, \
                k.client_visible \
           FROM prj_tasks k JOIN sd_tickets t ON t.id = k.ticket_id JOIN sd_statuses s ON s.id = t.status_id \
           LEFT JOIN users u ON u.id = t.assignee_user_id \
          WHERE k.project_id = $1 ORDER BY k.position, t.number",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok((phases, milestones, tasks))
}

fn assemble(
    row: Row,
    phases: Vec<ProjectPhase>,
    milestones: Vec<Milestone>,
    tasks: Vec<ProjectTask>,
) -> Project {
    let progress = Progress {
        done: i64::try_from(
            tasks
                .iter()
                .filter(|t| matches!(t.status_category.as_str(), "resolved" | "closed"))
                .count(),
        )
        .unwrap_or(0),
        total: i64::try_from(tasks.len()).unwrap_or(0),
    };
    let health = health(
        &row,
        &milestones,
        &progress,
        OffsetDateTime::now_utc().date(),
    );
    Project {
        burn_percent: burn(row.budget_hours, row.used_hours),
        id: row.id,
        org_id: row.org_id,
        org_name: row.org_name,
        name: row.name,
        description: row.description,
        manager_user_id: row.manager_user_id,
        manager_name: row.manager_name,
        status: row.status,
        start_on: row.start_on,
        end_on: row.end_on,
        budget_hours: row.budget_hours,
        used_hours: (row.used_hours * 100.0).round() / 100.0,
        billing_method: row.billing_method,
        thresholds: row.thresholds,
        thresholds_fired: row.thresholds_fired,
        portal_visible: row.portal_visible,
        template_id: row.template_id,
        health,
        progress,
        phases,
        milestones,
        tasks,
        version: row.version,
    }
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<Project> {
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE p.id = $1")))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(DeskError::NotFound)?;
    let (phases, milestones, tasks) = details(&mut *conn, id).await?;
    Ok(assemble(row, phases, milestones, tasks))
}

/// Projects in the transaction's scope (optionally one company's), newest
/// first.
pub async fn list(conn: &mut PgConnection, org_id: Option<Uuid>) -> DeskResult<Vec<Project>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR p.org_id = $1) ORDER BY p.created_at DESC LIMIT 500"
    )))
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let (phases, milestones, tasks) = details(&mut *conn, row.id).await?;
        out.push(assemble(row, phases, milestones, tasks));
    }
    Ok(out)
}

fn check_thresholds(t: &[i16]) -> DeskResult<Vec<i16>> {
    if t.len() > 10 || t.iter().any(|v| !(1..=500).contains(v)) {
        return Err(DeskError::invalid(
            "thresholds",
            "up to 10 percentages, 1 to 500",
        ));
    }
    let mut v = t.to_vec();
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

async fn emit(
    conn: &mut PgConnection,
    event: &'static str,
    project: Uuid,
    org: Uuid,
    payload: Json,
) -> DeskResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(org),
            subject_type: "project",
            subject_id: project,
            payload,
        },
    )
    .await?;
    Ok(())
}

/// A task ticket in a project.
#[allow(clippy::too_many_arguments)]
async fn task_ticket(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    project: (Uuid, Uuid),
    prefix: &str,
    title: &str,
    description: &str,
    assignee: Option<Uuid>,
    place: (Option<Uuid>, Option<f64>, i32, bool),
) -> DeskResult<Uuid> {
    let (project_id, org) = project;
    let (phase, estimate, position, client_visible) = place;
    let ticket = tickets::create(
        conn,
        ctx,
        &NewTicket {
            org_id: org,
            r#type: Some("project_task".into()),
            title: title.to_owned(),
            description: (!description.is_empty()).then(|| description.to_owned()),
            assignee_user_id: assignee,
            source: Some("project".into()),
            ..NewTicket::default()
        },
        prefix,
        None,
    )
    .await?;
    sqlx::query(
        "INSERT INTO prj_tasks (ticket_id, project_id, org_id, phase_id, estimate_hours, position, client_visible) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ticket.id)
    .bind(project_id)
    .bind(org)
    .bind(phase)
    .bind(estimate)
    .bind(position)
    .bind(client_visible)
    .execute(&mut *conn)
    .await?;
    Ok(ticket.id)
}

/// Creates a project, laid out from its template if given: phases,
/// milestones (due their offset after the start), and task tickets
/// (assigned as `assignees` says). Raises `project.created`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &NewProject,
) -> DeskResult<Project> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(DeskError::invalid("name", "1 to 200 characters"));
    }
    let start = parse_date(&input.start_on, "startOn")?;
    let end = input
        .end_on
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| parse_date(s, "endOn"))
        .transpose()?;
    if end.is_some_and(|e| e < start) {
        return Err(DeskError::invalid("endOn", "on or after the start"));
    }
    let billing = input
        .billing_method
        .clone()
        .unwrap_or_else(|| "time_materials".into());
    if !BILLING.contains(&billing.as_str()) {
        return Err(DeskError::invalid("billingMethod", BILLING.join(", ")));
    }
    let template = match input.template_id {
        Some(t) => Some(project_templates::get(&mut *conn, t).await?),
        None => None,
    };
    if let Some(key) = input.assignees.keys().find(|k| {
        !template
            .as_ref()
            .is_some_and(|t| t.structure.tasks().any(|(_, x)| &x.key == *k))
    }) {
        return Err(DeskError::invalid(
            "assignees",
            format!("the template has no task {key:?}"),
        ));
    }
    let budget = input.budget_hours.or_else(|| {
        template
            .as_ref()
            .map(|t| {
                t.structure
                    .phases
                    .iter()
                    .filter_map(|p| p.budget_hours)
                    .sum::<f64>()
            })
            .filter(|b| *b > 0.0)
    });
    let thresholds = check_thresholds(input.thresholds.as_deref().unwrap_or(&[50, 75, 90, 100]))?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO prj_projects (id, org_id, name, description, manager_user_id, start_on, end_on, budget_hours, \
                                   billing_method, template_id, thresholds, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(input.org_id)
    .bind(name)
    .bind(input.description.trim())
    .bind(input.manager_user_id)
    .bind(start)
    .bind(end)
    .bind(budget)
    .bind(&billing)
    .bind(input.template_id)
    .bind(&thresholds)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if let Some(t) = &template {
        lay_out(
            conn,
            ctx,
            (id, input.org_id),
            start,
            &t.structure,
            &input.assignees,
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.created")
            .org(input.org_id)
            .target("project", id),
    )
    .await?;
    emit(
        &mut *conn,
        "project.created",
        id,
        input.org_id,
        json!({ "name": name, "templateId": input.template_id }),
    )
    .await?;
    get(conn, id).await
}

/// Phases, milestones and tasks from a template.
async fn lay_out(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    (id, org): (Uuid, Uuid),
    start: Date,
    structure: &project_templates::Structure,
    assignees: &BTreeMap<String, Uuid>,
) -> DeskResult<()> {
    let prefix = tickets::prefix_for(&mut *conn, org).await?;
    let mut phase_ids = Vec::new();
    for (i, p) in structure.phases.iter().enumerate() {
        let pid = Uuid::now_v7();
        sqlx::query("INSERT INTO prj_phases (id, project_id, org_id, name, position, budget_hours) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(pid)
            .bind(id)
            .bind(org)
            .bind(&p.name)
            .bind(i32::try_from(i).unwrap_or(0))
            .bind(p.budget_hours)
            .execute(&mut *conn)
            .await?;
        phase_ids.push(pid);
    }
    for (i, m) in structure.milestones.iter().enumerate() {
        sqlx::query(
            "INSERT INTO prj_milestones (id, project_id, org_id, name, due_on, client_visible, position) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(org)
        .bind(&m.name)
        .bind(start + Duration::days(i64::from(m.offset_days)))
        .bind(m.client_visible)
        .bind(i32::try_from(i).unwrap_or(0))
        .execute(&mut *conn)
        .await?;
    }
    let mut made: BTreeMap<&str, Uuid> = BTreeMap::new();
    for (n, (phase, t)) in structure.tasks().enumerate() {
        let ticket = task_ticket(
            conn,
            ctx,
            (id, org),
            &prefix,
            &t.title,
            &t.description,
            assignees.get(&t.key).copied(),
            (
                phase_ids.get(phase).copied(),
                t.estimate_hours,
                i32::try_from(n).unwrap_or(0),
                t.client_visible,
            ),
        )
        .await?;
        made.insert(t.key.as_str(), ticket);
    }
    for (_, t) in structure.tasks().filter(|(_, t)| !t.depends_on.is_empty()) {
        let deps: Vec<Uuid> = t
            .depends_on
            .iter()
            .filter_map(|d| made.get(d.as_str()).copied())
            .collect();
        sqlx::query("UPDATE prj_tasks SET depends_on = $2 WHERE ticket_id = $1")
            .bind(made[t.key.as_str()])
            .bind(&deps)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Changes a project (send the version you read).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    c: &ProjectChanges,
) -> DeskResult<Project> {
    let current = get(&mut *conn, id).await?;
    if c.version != current.version {
        return Err(DeskError::VersionConflict);
    }
    let name = c
        .name
        .as_deref()
        .map_or_else(|| current.name.clone(), |n| n.trim().to_owned());
    if name.is_empty() || name.chars().count() > 200 {
        return Err(DeskError::invalid("name", "1 to 200 characters"));
    }
    let status = c.status.clone().unwrap_or(current.status.clone());
    if !STATUSES.contains(&status.as_str()) {
        return Err(DeskError::invalid("status", STATUSES.join(", ")));
    }
    let billing = c
        .billing_method
        .clone()
        .unwrap_or(current.billing_method.clone());
    if !BILLING.contains(&billing.as_str()) {
        return Err(DeskError::invalid("billingMethod", BILLING.join(", ")));
    }
    let end = match &c.end_on {
        Some(e) if e.is_empty() => None,
        Some(e) => Some(parse_date(e, "endOn")?),
        None => current
            .end_on
            .as_deref()
            .map(|e| parse_date(e, "endOn"))
            .transpose()?,
    };
    let thresholds = match &c.thresholds {
        Some(t) => check_thresholds(t)?,
        None => current.thresholds.clone(),
    };
    sqlx::query(
        "UPDATE prj_projects SET name = $2, description = $3, manager_user_id = $4, status = $5, end_on = $6, \
                budget_hours = $7, billing_method = $8, thresholds = $9, portal_visible = $10, \
                thresholds_fired = (SELECT coalesce(array_agg(f), '{}') FROM unnest(thresholds_fired) f WHERE f = ANY($9)), \
                version = version + 1, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(c.description.as_deref().map_or_else(|| current.description.clone(), |d| d.trim().to_owned()))
    .bind(c.manager_user_id.or(current.manager_user_id))
    .bind(&status)
    .bind(end)
    .bind(c.budget_hours.or(current.budget_hours))
    .bind(&billing)
    .bind(&thresholds)
    .bind(c.portal_visible.unwrap_or(current.portal_visible))
    .execute(&mut *conn)
    .await?;
    if status != current.status {
        emit(
            &mut *conn,
            "project.status_changed",
            id,
            current.org_id,
            json!({ "name": name, "from": current.status, "to": status }),
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.updated")
            .org(current.org_id)
            .target("project", id),
    )
    .await?;
    // A bigger budget can bring burn back under thresholds already raised.
    check_budget(&mut *conn, id).await?;
    get(conn, id).await
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewTask {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub phase_id: Option<Uuid>,
    #[serde(default)]
    pub assignee_user_id: Option<Uuid>,
    #[serde(default)]
    pub estimate_hours: Option<f64>,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub client_visible: bool,
}

/// Adds a task (a ticket) to a project.
pub async fn add_task(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &NewTask,
) -> DeskResult<Project> {
    let project = get(&mut *conn, id).await?;
    if input
        .phase_id
        .is_some_and(|p| !project.phases.iter().any(|x| x.id == p))
    {
        return Err(DeskError::invalid("phaseId", "not a phase of this project"));
    }
    if input
        .depends_on
        .iter()
        .any(|d| !project.tasks.iter().any(|t| t.ticket_id == *d))
    {
        return Err(DeskError::invalid("dependsOn", "tasks of this project"));
    }
    let prefix = tickets::prefix_for(&mut *conn, project.org_id).await?;
    let position = i32::try_from(project.tasks.len()).unwrap_or(0);
    let ticket = task_ticket(
        conn,
        ctx,
        (id, project.org_id),
        &prefix,
        &input.title,
        &input.description,
        input.assignee_user_id,
        (
            input.phase_id,
            input.estimate_hours,
            position,
            input.client_visible,
        ),
    )
    .await?;
    sqlx::query("UPDATE prj_tasks SET depends_on = $2 WHERE ticket_id = $1")
        .bind(ticket)
        .bind(&input.depends_on)
        .execute(&mut *conn)
        .await?;
    get(conn, id).await
}

/// Marks a milestone reached (or not). Reaching one raises
/// `project.milestone_reached`.
pub async fn set_milestone(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    milestone: Uuid,
    reached: bool,
) -> DeskResult<Project> {
    let row: Option<(String, Option<OffsetDateTime>, Uuid)> = sqlx::query_as(
        "SELECT name, reached_at, org_id FROM prj_milestones WHERE id = $1 AND project_id = $2",
    )
    .bind(milestone)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let (name, was, org) = row.ok_or(DeskError::NotFound)?;
    sqlx::query("UPDATE prj_milestones SET reached_at = CASE WHEN $2 THEN coalesce(reached_at, now()) END WHERE id = $1")
        .bind(milestone)
        .bind(reached)
        .execute(&mut *conn)
        .await?;
    if reached && was.is_none() {
        emit(
            &mut *conn,
            "project.milestone_reached",
            id,
            org,
            json!({ "milestone": name, "milestoneId": milestone }),
        )
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("project.milestone")
            .org(org)
            .target("project", id)
            .details(json!({ "milestone": name, "reached": reached })),
    )
    .await?;
    get(conn, id).await
}

/// A project's budget state: org, name, budget hours, used hours,
/// thresholds and those raised.
type BudgetRow = (Uuid, String, Option<f64>, f64, Vec<i16>, Vec<i16>);

/// Raises `project.budget_threshold` for each threshold the project's burn
/// has reached that wasn't raised yet (and forgets thresholds it has
/// dropped back under). Called after time changes on its tasks.
pub async fn check_budget(conn: &mut PgConnection, project: Uuid) -> DeskResult<Vec<i16>> {
    let row: Option<BudgetRow> = sqlx::query_as(
        "SELECT p.org_id, p.name, p.budget_hours::float8, \
                coalesce((SELECT sum(e.minutes) FROM sd_time_entries e JOIN prj_tasks k ON k.ticket_id = e.ticket_id \
                          WHERE k.project_id = p.id), 0)::float8 / 60, p.thresholds, p.thresholds_fired \
           FROM prj_projects p WHERE p.id = $1",
    )
    .bind(project)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((org, name, budget, used, thresholds, fired)) = row else {
        return Ok(Vec::new());
    };
    let Some(percent) = burn(budget, used) else {
        return Ok(Vec::new());
    };
    let reached: Vec<i16> = thresholds
        .iter()
        .copied()
        .filter(|t| percent >= f64::from(*t))
        .collect();
    let new: Vec<i16> = reached
        .iter()
        .copied()
        .filter(|t| !fired.contains(t))
        .collect();
    if reached != fired {
        sqlx::query("UPDATE prj_projects SET thresholds_fired = $2 WHERE id = $1")
            .bind(project)
            .bind(&reached)
            .execute(&mut *conn)
            .await?;
    }
    for t in &new {
        emit(
            &mut *conn,
            "project.budget_threshold",
            project,
            org,
            json!({ "name": name, "threshold": t, "percent": percent, "usedHours": (used * 100.0).round() / 100.0, "budgetHours": budget }),
        )
        .await?;
    }
    Ok(new)
}

/// After time changes on a ticket: its project's budget check, if it's a
/// project task.
pub async fn after_time(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<()> {
    let project: Option<Uuid> =
        sqlx::query_scalar("SELECT project_id FROM prj_tasks WHERE ticket_id = $1")
            .bind(ticket)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(p) = project {
        check_budget(conn, p).await?;
    }
    Ok(())
}

/// After a ticket's status changes: `project.task_completed` when a
/// project task is done.
pub async fn after_status(conn: &mut PgConnection, ticket: Uuid, category: &str) -> DeskResult<()> {
    if category != "resolved" {
        return Ok(());
    }
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT k.project_id, k.org_id, t.title FROM prj_tasks k JOIN sd_tickets t ON t.id = k.ticket_id WHERE k.ticket_id = $1",
    )
    .bind(ticket)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((project, org, title)) = row {
        emit(
            conn,
            "project.task_completed",
            project,
            org,
            json!({ "taskId": ticket, "title": title }),
        )
        .await?;
    }
    Ok(())
}

/// What a client's portal shows of a project.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalProject {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub status: String,
    pub start_on: String,
    pub end_on: Option<String>,
    pub progress: Progress,
    pub milestones: Vec<Milestone>,
    /// Tasks marked for the client.
    pub tasks: Vec<PortalTask>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalTask {
    pub title: String,
    pub status_name: String,
    pub done: bool,
}

/// A company's projects as its portal shows them.
pub async fn for_portal(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<Vec<PortalProject>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM prj_projects WHERE org_id = $1 AND portal_visible AND status <> 'cancelled' ORDER BY start_on DESC",
    )
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for id in ids {
        let p = get(&mut *conn, id).await?;
        out.push(PortalProject {
            id: p.id,
            name: p.name,
            description: p.description,
            status: p.status,
            start_on: p.start_on,
            end_on: p.end_on,
            progress: p.progress,
            milestones: p
                .milestones
                .into_iter()
                .filter(|m| m.client_visible)
                .collect(),
            tasks: p
                .tasks
                .into_iter()
                .filter(|t| t.client_visible)
                .map(|t| PortalTask {
                    done: matches!(t.status_category.as_str(), "resolved" | "closed"),
                    title: t.title,
                    status_name: t.status_name,
                })
                .collect(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_burn() {
        assert_eq!(
            parse_date("2026-12-01", "x").unwrap().to_string(),
            "2026-12-01"
        );
        assert!(parse_date("12/01/2026", "x").is_err());
        assert_eq!(burn(Some(40.0), 30.0), Some(75.0));
        assert_eq!(burn(None, 30.0), None);
        assert_eq!(check_thresholds(&[90, 50, 50]).unwrap(), vec![50, 90]);
        assert!(check_thresholds(&[0]).is_err());
    }
}
