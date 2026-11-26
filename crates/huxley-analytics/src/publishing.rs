//! Publishing dashboards (ADR-045): a personal dashboard is submitted for
//! an audience (teams, roles, the client portal); someone with
//! `dashboard:manage` approves it, and it's shared with that audience as
//! it was submitted, or rejects it with a comment. Approvals are numbered
//! publications, kept with what was published.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::dashboards::{self, DashboardInput, Widget};
use crate::{AnalyticsError, AnalyticsResult};

fn bad(at: &str, message: impl Into<String>) -> AnalyticsError {
    AnalyticsError::Invalid(at.to_owned(), message.into())
}

fn audit_err(e: impl std::fmt::Display) -> AnalyticsError {
    AnalyticsError::Db(sqlx::Error::Protocol(e.to_string()))
}

/// What was submitted.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = DashboardSnapshot)]
pub struct Snapshot {
    pub name: String,
    pub description: Option<String>,
    pub range: String,
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[schema(as = SubmissionStatus)]
pub enum Status {
    Pending,
    Approved,
    Rejected,
    Withdrawn,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = DashboardSubmission)]
pub struct Submission {
    pub id: Uuid,
    pub dashboard_id: Uuid,
    pub org_id: Uuid,
    pub submitted_by: Option<Uuid>,
    pub submitted_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    pub teams: Vec<Uuid>,
    pub roles: Vec<Uuid>,
    pub portal: bool,
    pub note: Option<String>,
    #[sqlx(json)]
    pub snapshot: Snapshot,
    pub status: Status,
    pub decided_by: Option<Uuid>,
    pub decided_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub comment: Option<String>,
    /// The publication's number, once approved.
    pub number: Option<i32>,
}

const SELECT: &str = "SELECT s.id, s.dashboard_id, s.org_id, s.submitted_by, su.display_name AS submitted_by_name, \
     s.submitted_at, s.teams, s.roles, s.portal, s.note, s.snapshot, s.status, s.decided_by, \
     du.display_name AS decided_by_name, s.decided_at, s.comment, s.number \
     FROM dashboard_submissions s LEFT JOIN users su ON su.id = s.submitted_by LEFT JOIN users du ON du.id = s.decided_by";

/// The audience asked for.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SubmitDashboard)]
pub struct SubmitInput {
    #[serde(default)]
    pub teams: Vec<Uuid>,
    #[serde(default)]
    pub roles: Vec<Uuid>,
    /// Show it in the client portal too.
    #[serde(default)]
    pub portal: bool,
    /// For the reviewer.
    #[serde(default)]
    pub note: Option<String>,
}

async fn one(conn: &mut PgConnection, id: Uuid) -> AnalyticsResult<Option<Submission>> {
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE s.id = $1")))
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Submits `user`'s personal dashboard for publishing, as it is now.
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    dashboard_id: Uuid,
    input: &SubmitInput,
) -> AnalyticsResult<Option<Submission>> {
    let Some(d) = dashboards::get_any(&mut *conn, dashboard_id).await? else {
        return Ok(None);
    };
    if d.owner_user_id != Some(user) {
        return Err(bad(
            "dashboard",
            "only your own personal dashboards can be submitted",
        ));
    }
    let org_id = d
        .org_id
        .ok_or_else(|| bad("dashboard", "not a stored dashboard"))?;
    let snapshot = Snapshot {
        name: d.name.clone(),
        description: d.description.clone(),
        range: d.range.clone(),
        widgets: d.widgets,
    };
    dashboards::check(&DashboardInput {
        name: snapshot.name.clone(),
        description: snapshot.description.clone(),
        range: Some(snapshot.range.clone()),
        widgets: snapshot.widgets.clone(),
        portal: false,
        shared: None,
        version: None,
    })?;
    if snapshot.widgets.is_empty() {
        return Err(bad("widgets", "add a widget before submitting it"));
    }
    if input.portal {
        dashboards::check_portal(&snapshot.widgets)?;
    }
    check_audience(&mut *conn, input).await?;
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.chars().count() > 1000) {
        return Err(bad("note", "at most 1000 characters"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO dashboard_submissions (id, dashboard_id, org_id, submitted_by, teams, roles, portal, note, snapshot) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(dashboard_id)
    .bind(org_id)
    .bind(user)
    .bind(&input.teams)
    .bind(&input.roles)
    .bind(input.portal)
    .bind(note)
    .bind(sqlx::types::Json(&snapshot))
    .execute(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("dashboard_submissions_pending") => {
            bad("dashboard", "it's already waiting for approval")
        }
        _ => AnalyticsError::Db(e),
    })?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("dashboard.submitted")
            .org(org_id)
            .target("dashboard", dashboard_id)
            .details(json!({ "submission": id })),
    )
    .await
    .map_err(audit_err)?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "dashboard.submitted",
            org_id: Some(org_id),
            subject_type: "dashboard",
            subject_id: dashboard_id,
            payload: json!({ "submission_id": id, "name": snapshot.name, "submitted_by": user, "portal": input.portal }),
        },
    )
    .await
    .map_err(audit_err)?;
    one(conn, id).await
}

/// The teams and roles must exist.
async fn check_audience(conn: &mut PgConnection, input: &SubmitInput) -> AnalyticsResult<()> {
    let teams: i64 = sqlx::query_scalar("SELECT count(*) FROM teams WHERE id = ANY($1)")
        .bind(&input.teams)
        .fetch_one(&mut *conn)
        .await?;
    if usize::try_from(teams).unwrap_or(0) != dedup(&input.teams) {
        return Err(bad("teams", "one of those teams doesn't exist"));
    }
    let roles: i64 = sqlx::query_scalar("SELECT count(*) FROM roles WHERE id = ANY($1)")
        .bind(&input.roles)
        .fetch_one(conn)
        .await?;
    if usize::try_from(roles).unwrap_or(0) != dedup(&input.roles) {
        return Err(bad("roles", "one of those roles doesn't exist"));
    }
    Ok(())
}

fn dedup(ids: &[Uuid]) -> usize {
    let mut v = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// Approves (publishing what was submitted to its audience) or rejects a
/// pending submission. `Ok(None)`: no such pending submission.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    approve: bool,
    comment: Option<&str>,
) -> AnalyticsResult<Option<Submission>> {
    let comment = comment.map(str::trim).filter(|c| !c.is_empty());
    if comment.is_some_and(|c| c.chars().count() > 1000) {
        return Err(bad("comment", "at most 1000 characters"));
    }
    if !approve && comment.is_none() {
        return Err(bad("comment", "say what to change"));
    }
    let Some(s) = one(&mut *conn, id)
        .await?
        .filter(|s| s.status == Status::Pending)
    else {
        return Ok(None);
    };
    let number: Option<i32> = if approve {
        let n: i32 = sqlx::query_scalar(
            "SELECT coalesce(max(number), 0) + 1 FROM dashboard_submissions WHERE dashboard_id = $1",
        )
        .bind(s.dashboard_id)
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE dashboards SET owner_user_id = NULL, name = $2, description = $3, range = $4, widgets = $5, \
                    portal = $6, audience_teams = $7, audience_roles = $8, version = version + 1, updated_at = now() \
              WHERE id = $1",
        )
        .bind(s.dashboard_id)
        .bind(&s.snapshot.name)
        .bind(&s.snapshot.description)
        .bind(&s.snapshot.range)
        .bind(sqlx::types::Json(&s.snapshot.widgets))
        .bind(s.portal)
        .bind(&s.teams)
        .bind(&s.roles)
        .execute(&mut *conn)
        .await?;
        Some(n)
    } else {
        None
    };
    sqlx::query(
        "UPDATE dashboard_submissions SET status = $2, decided_by = $3, decided_at = now(), comment = $4, number = $5 \
          WHERE id = $1",
    )
    .bind(id)
    .bind(if approve { Status::Approved } else { Status::Rejected })
    .bind(ctx.actor.id())
    .bind(comment)
    .bind(number)
    .execute(&mut *conn)
    .await?;
    let kind = if approve {
        "dashboard.published"
    } else {
        "dashboard.rejected"
    };
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(kind)
            .org(s.org_id)
            .target("dashboard", s.dashboard_id)
            .details(json!({ "submission": id, "number": number })),
    )
    .await
    .map_err(audit_err)?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: kind,
            org_id: Some(s.org_id),
            subject_type: "dashboard",
            subject_id: s.dashboard_id,
            payload: json!({
                "submission_id": id, "name": s.snapshot.name, "submitted_by": s.submitted_by,
                "comment": comment, "number": number, "portal": s.portal,
            }),
        },
    )
    .await
    .map_err(audit_err)?;
    one(conn, id).await
}

/// Withdraws `user`'s pending submission.
pub async fn withdraw(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    id: Uuid,
) -> AnalyticsResult<bool> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE dashboard_submissions SET status = 'withdrawn', decided_by = $2, decided_at = now() \
          WHERE id = $1 AND submitted_by = $2 AND status = 'pending' RETURNING org_id, dashboard_id",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((org, dashboard)) = row {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("dashboard.withdrawn")
                .org(org)
                .target("dashboard", dashboard),
        )
        .await
        .map_err(audit_err)?;
    }
    Ok(row.is_some())
}

/// Pending submissions in the transaction's scope, oldest first.
pub async fn pending(conn: &mut PgConnection) -> AnalyticsResult<Vec<Submission>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.status = 'pending' ORDER BY s.submitted_at"
    )))
    .fetch_all(conn)
    .await?)
}

/// A dashboard's submissions, newest first.
pub async fn for_dashboard(
    conn: &mut PgConnection,
    dashboard_id: Uuid,
) -> AnalyticsResult<Vec<Submission>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.dashboard_id = $1 ORDER BY s.submitted_at DESC"
    )))
    .bind(dashboard_id)
    .fetch_all(conn)
    .await?)
}

/// A team or role to publish to.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AudienceOption {
    pub id: Uuid,
    pub name: String,
}

/// What a dashboard can be published to.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Audiences {
    pub teams: Vec<AudienceOption>,
    pub roles: Vec<AudienceOption>,
}

pub async fn audiences(conn: &mut PgConnection) -> AnalyticsResult<Audiences> {
    let teams = sqlx::query_as("SELECT id, name FROM teams ORDER BY lower(name)")
        .fetch_all(&mut *conn)
        .await?;
    let roles = sqlx::query_as("SELECT id, name FROM roles ORDER BY is_system DESC, lower(name)")
        .fetch_all(conn)
        .await?;
    Ok(Audiences { teams, roles })
}

/// One submission (to check who may decide it).
pub async fn get(conn: &mut PgConnection, id: Uuid) -> AnalyticsResult<Option<Submission>> {
    one(conn, id).await
}
