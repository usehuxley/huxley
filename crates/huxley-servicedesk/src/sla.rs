//! SLAs (ADR-037): policies with targets per priority, where they apply,
//! and each ticket's response and resolution clocks.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::calendar::Calendar;
use crate::tickets::PRIORITIES;
use crate::{DeskError, DeskResult, text};

pub const HOURS_SOURCES: &[&str] = &["client_site", "policy", "always"];

/// A priority's targets, in minutes of business time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub response: Option<i32>,
    pub resolution: Option<i32>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// `client_site`, `policy` or `always`.
    pub hours_source: String,
    /// `{"mon": [["09:00", "17:00"]], …}`.
    #[schema(value_type = Object)]
    pub hours: Json,
    /// For the policy's hours; none: the organization's time zone.
    pub timezone: Option<String>,
    pub is_default: bool,
    /// Targets by priority (`p1` … `p4`).
    #[sqlx(skip)]
    pub targets: BTreeMap<String, Target>,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `client_site` (default), `policy` or `always`.
    #[serde(default)]
    pub hours_source: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub hours: Option<Json>,
    #[serde(default)]
    pub timezone: Option<String>,
    /// Make it the default policy.
    #[serde(default)]
    pub is_default: bool,
    /// Targets by priority; priorities left out have none.
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

async fn targets_of(conn: &mut PgConnection, policy: Uuid) -> DeskResult<BTreeMap<String, Target>> {
    let rows: Vec<(String, Option<i32>, Option<i32>)> = sqlx::query_as(
        "SELECT priority, response_minutes, resolution_minutes FROM sd_sla_targets \
          WHERE policy_id = $1 ORDER BY priority",
    )
    .bind(policy)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(p, response, resolution)| {
            (
                p,
                Target {
                    response,
                    resolution,
                },
            )
        })
        .collect())
}

const SELECT: &str = "SELECT id, name, description, hours_source, hours, timezone, is_default, version \
    FROM sd_sla_policies";

/// Active policies, the default first.
pub async fn policies(conn: &mut PgConnection) -> DeskResult<Vec<Policy>> {
    let mut list: Vec<Policy> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE archived_at IS NULL ORDER BY is_default DESC, lower(name)"
    )))
    .fetch_all(&mut *conn)
    .await?;
    for p in &mut list {
        p.targets = targets_of(conn, p.id).await?;
    }
    Ok(list)
}

pub async fn policy(conn: &mut PgConnection, id: Uuid) -> DeskResult<Policy> {
    let mut p: Policy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE id = $1 AND archived_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(DeskError::NotFound)?;
    p.targets = targets_of(conn, id).await?;
    Ok(p)
}

struct Checked {
    name: String,
    description: String,
    hours_source: String,
    hours: Json,
    timezone: Option<String>,
}

fn check(input: &PolicyInput) -> DeskResult<Checked> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| DeskError::invalid("name", "a policy needs a name"))?;
    let hours_source = input
        .hours_source
        .clone()
        .unwrap_or_else(|| "client_site".into());
    if !HOURS_SOURCES.contains(&hours_source.as_str()) {
        return Err(DeskError::invalid("hoursSource", HOURS_SOURCES.join(", ")));
    }
    let hours = match &input.hours {
        Some(h) => huxley_crm::sites::check_hours(h).map_err(|e| match e {
            huxley_crm::CrmError::Invalid { message, .. } => DeskError::invalid("hours", message),
            other => DeskError::invalid("hours", other.to_string()),
        })?,
        None => json!({ "mon": [["09:00", "17:00"]], "tue": [["09:00", "17:00"]],
                        "wed": [["09:00", "17:00"]], "thu": [["09:00", "17:00"]],
                        "fri": [["09:00", "17:00"]] }),
    };
    let timezone = text("timezone", input.timezone.as_deref(), 64)?;
    if let Some(tz) = &timezone
        && jiff::tz::TimeZone::get(tz).is_err()
    {
        return Err(DeskError::invalid(
            "timezone",
            format!("{tz:?} isn't a time zone"),
        ));
    }
    if hours_source == "policy"
        && Calendar::weekly(timezone.as_deref().unwrap_or("UTC"), &hours).is_none()
    {
        return Err(DeskError::invalid(
            "hours",
            "the policy's hours need some open time",
        ));
    }
    for (priority, target) in &input.targets {
        if !PRIORITIES.contains(&priority.as_str()) {
            return Err(DeskError::invalid(
                "targets",
                format!("{priority:?} isn't p1 to p4"),
            ));
        }
        for minutes in [target.response, target.resolution].into_iter().flatten() {
            if !(1..=525_600).contains(&minutes) {
                return Err(DeskError::invalid(
                    "targets",
                    "minutes are 1 to 525600 (a year)",
                ));
            }
        }
    }
    Ok(Checked {
        name,
        description: text("description", input.description.as_deref(), 2000)?.unwrap_or_default(),
        hours_source,
        hours,
        timezone,
    })
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_sla_policies_name") => {
            DeskError::invalid("name", "another policy has this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

async fn save_targets(
    conn: &mut PgConnection,
    id: Uuid,
    targets: &BTreeMap<String, Target>,
) -> DeskResult<()> {
    sqlx::query("DELETE FROM sd_sla_targets WHERE policy_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for (priority, t) in targets {
        if t.response.is_none() && t.resolution.is_none() {
            continue;
        }
        sqlx::query(
            "INSERT INTO sd_sla_targets (policy_id, priority, response_minutes, resolution_minutes) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(priority)
        .bind(t.response)
        .bind(t.resolution)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn make_default(conn: &mut PgConnection, id: Uuid) -> DeskResult<()> {
    sqlx::query("UPDATE sd_sla_policies SET is_default = false WHERE is_default AND id <> $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE sd_sla_policies SET is_default = true WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &PolicyInput,
) -> DeskResult<Policy> {
    let c = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_sla_policies (id, name, description, hours_source, hours, timezone) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.description)
    .bind(&c.hours_source)
    .bind(&c.hours)
    .bind(&c.timezone)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    save_targets(conn, id, &input.targets).await?;
    if input.is_default {
        make_default(conn, id).await?;
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sla_policy.created")
            .target("sla_policy", id)
            .details(json!({ "name": c.name, "targets": input.targets })),
    )
    .await?;
    policy(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &PolicyInput,
) -> DeskResult<Policy> {
    let c = check(input)?;
    let before = policy(conn, id).await?;
    if before.is_default && !input.is_default {
        return Err(DeskError::invalid(
            "isDefault",
            "make another policy the default instead",
        ));
    }
    let rows = sqlx::query(
        "UPDATE sd_sla_policies SET name = $3, description = $4, hours_source = $5, hours = $6, \
                timezone = $7, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&c.name)
    .bind(&c.description)
    .bind(&c.hours_source)
    .bind(&c.hours)
    .bind(&c.timezone)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    save_targets(conn, id, &input.targets).await?;
    if input.is_default {
        make_default(conn, id).await?;
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sla_policy.updated")
            .target("sla_policy", id)
            .details(json!({ "name": c.name, "before": before.targets, "after": input.targets })),
    )
    .await?;
    policy(conn, id).await
}

/// Archives a policy (not the default). Organizations assigned to it fall
/// back to the next one up; tickets keep their terms.
pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let p = policy(conn, id).await?;
    if p.is_default {
        return Err(DeskError::invalid(
            "isDefault",
            "the default policy can't be archived",
        ));
    }
    sqlx::query(
        "UPDATE sd_sla_policies SET archived_at = now(), version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM sd_sla_assignments WHERE policy_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sla_policy.archived")
            .target("sla_policy", id)
            .details(json!({ "name": p.name })),
    )
    .await?;
    Ok(())
}

/// The policy for an organization: where it's set, and which applies.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlaAssignment {
    /// Set on this organization (none: it follows the one above).
    pub policy_id: Option<Uuid>,
    /// The policy that applies here, if any.
    pub effective_policy_id: Option<Uuid>,
    pub effective_policy_name: Option<String>,
    /// Where the effective policy comes from: `here`, `above` or `default`.
    pub source: Option<String>,
}

/// The policy that applies at an organization (needs system scope: it
/// looks at the organizations above).
async fn effective(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> DeskResult<Option<(Uuid, &'static str)>> {
    let nearest: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT a.policy_id, a.org_id = t.id FROM sd_sla_assignments a \
           JOIN organizations o ON o.id = a.org_id \
           JOIN organizations t ON t.id = $1 \
           JOIN sd_sla_policies p ON p.id = a.policy_id AND p.archived_at IS NULL \
          WHERE t.path <@ o.path ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((id, here)) = nearest {
        return Ok(Some((id, if here { "here" } else { "above" })));
    }
    let default: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sd_sla_policies WHERE is_default AND archived_at IS NULL",
    )
    .fetch_optional(conn)
    .await?;
    Ok(default.map(|id| (id, "default")))
}

pub async fn assignment(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<SlaAssignment> {
    let set: Option<Uuid> =
        sqlx::query_scalar("SELECT policy_id FROM sd_sla_assignments WHERE org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut *conn)
            .await?;
    let eff = effective(conn, org_id).await?;
    let name = match eff {
        Some((id, _)) => Some(policy(conn, id).await?.name),
        None => None,
    };
    Ok(SlaAssignment {
        policy_id: set,
        effective_policy_id: eff.map(|(id, _)| id),
        effective_policy_name: name,
        source: eff.map(|(_, s)| s.to_owned()),
    })
}

/// Sets (or, with `None`, clears) the policy for an organization and
/// those below it.
pub async fn assign(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    policy_id: Option<Uuid>,
) -> DeskResult<SlaAssignment> {
    match policy_id {
        Some(p) => {
            policy(conn, p)
                .await
                .map_err(|_| DeskError::invalid("policyId", "there's no such policy"))?;
            sqlx::query(
                "INSERT INTO sd_sla_assignments (org_id, policy_id) VALUES ($1, $2) \
                 ON CONFLICT (org_id) DO UPDATE SET policy_id = EXCLUDED.policy_id",
            )
            .bind(org_id)
            .bind(p)
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM sd_sla_assignments WHERE org_id = $1")
                .bind(org_id)
                .execute(&mut *conn)
                .await?;
        }
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sla_policy.assigned")
            .org(org_id)
            .target("organization", org_id)
            .details(json!({ "policyId": policy_id })),
    )
    .await?;
    assignment(conn, org_id).await
}

/// The terms a new ticket at an organization gets.
#[derive(Debug, Clone)]
pub struct Plan {
    pub policy_id: Uuid,
    pub policy_name: String,
    pub calendar: Calendar,
    pub targets: BTreeMap<String, Target>,
}

/// The organization's time zone setting (system scope).
async fn org_timezone(conn: &mut PgConnection, org_id: Uuid) -> Option<String> {
    huxley_orgs::settings::resolve(conn, org_id)
        .await
        .ok()?
        .into_iter()
        .find(|s| s.def.key == huxley_orgs::settings::TIMEZONE)
        .and_then(|s| s.value)
}

/// The SLA terms for a new ticket at `org_id` (system scope), if any
/// policy applies.
pub async fn plan_for(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<Option<Plan>> {
    let Some((id, _)) = effective(conn, org_id).await? else {
        return Ok(None);
    };
    let p = policy(conn, id).await?;
    let calendar = match p.hours_source.as_str() {
        "always" => Calendar::Always,
        source => {
            let site: Option<(String, Json)> = if source == "client_site" {
                sqlx::query_as(
                    "SELECT timezone, hours FROM crm_sites \
                      WHERE org_id = $1 AND is_primary AND archived_at IS NULL",
                )
                .bind(org_id)
                .fetch_optional(&mut *conn)
                .await?
            } else {
                None
            };
            if let Some(c) = site.and_then(|(tz, hours)| Calendar::weekly(&tz, &hours)) {
                c
            } else {
                let tz = match p.timezone.clone() {
                    Some(tz) => tz,
                    None => org_timezone(conn, org_id)
                        .await
                        .unwrap_or_else(|| "UTC".into()),
                };
                Calendar::weekly(&tz, &p.hours).unwrap_or(Calendar::Always)
            }
        }
    };
    Ok(Some(Plan {
        policy_id: p.id,
        policy_name: p.name,
        calendar,
        targets: p.targets,
    }))
}

/// Starts a new ticket's clocks under `plan`.
pub async fn start(
    conn: &mut PgConnection,
    ticket: Uuid,
    org_id: Uuid,
    plan: &Plan,
) -> DeskResult<()> {
    sqlx::query(
        "INSERT INTO sd_ticket_sla (ticket_id, org_id, policy_id, policy_name, calendar, targets, \
                                    resolution_clock_at) \
         SELECT $1, $2, $3, $4, $5, $6, created_at FROM sd_tickets WHERE id = $1",
    )
    .bind(ticket)
    .bind(org_id)
    .bind(plan.policy_id)
    .bind(&plan.policy_name)
    .bind(plan.calendar.to_json())
    .bind(json!(plan.targets))
    .execute(&mut *conn)
    .await?;
    sync(conn, ticket).await
}

#[derive(sqlx::FromRow)]
struct ClockRow {
    calendar: Json,
    targets: Json,
    resolution_elapsed_s: i64,
    resolution_clock_at: Option<OffsetDateTime>,
    resolution_due_at: Option<OffsetDateTime>,
    resolution_warn_at: Option<OffsetDateTime>,
    priority: String,
    created_at: OffsetDateTime,
    first_response_at: Option<OffsetDateTime>,
    pauses_sla: bool,
    category: String,
}

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// Brings a ticket's clocks up to date after a change: accrues the
/// resolution clock, starts or stops it by status, and recomputes due and
/// warning times for its priority. No-op for tickets without an SLA.
pub async fn sync(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<()> {
    let row: Option<ClockRow> = sqlx::query_as(
        "SELECT l.calendar, l.targets, l.resolution_elapsed_s, l.resolution_clock_at, \
                l.resolution_due_at, l.resolution_warn_at, t.priority, t.created_at, \
                t.first_response_at, s.pauses_sla, s.category \
           FROM sd_ticket_sla l JOIN sd_tickets t ON t.id = l.ticket_id \
           JOIN sd_statuses s ON s.id = t.status_id \
          WHERE l.ticket_id = $1 FOR UPDATE OF l",
    )
    .bind(ticket)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(()) };
    let calendar = Calendar::from_json(&row.calendar);
    let target: Target = row
        .targets
        .get(&row.priority)
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .unwrap_or_default();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let created = row.created_at.unix_timestamp();

    // Response: from creation, in business time, never paused.
    let (response_due, response_warn, response_met) = match target.response {
        Some(minutes) => {
            let secs = i64::from(minutes) * 60;
            let due = calendar.add(created, secs);
            let met = row.first_response_at.map(|t| t.unix_timestamp() <= due);
            (
                Some(at(due)),
                Some(at(calendar.add(created, secs * 3 / 4))),
                met,
            )
        }
        None => (None, None, None),
    };

    // Resolution: accrue, then run only in working statuses.
    let mut elapsed = row.resolution_elapsed_s;
    if let Some(since) = row.resolution_clock_at {
        elapsed += calendar.open_between(since.unix_timestamp(), now);
    }
    let stopped = matches!(row.category.as_str(), "resolved" | "closed");
    let running = !stopped && !row.pauses_sla;
    let (due, warn, met) = match target.resolution {
        Some(minutes) => {
            let secs = i64::from(minutes) * 60;
            let left = |share: i64| {
                let remaining = share - elapsed;
                if remaining > 0 {
                    Some(at(calendar.add(now, remaining)))
                } else {
                    None
                }
            };
            if running {
                (
                    left(secs).or(row.resolution_due_at).or(Some(at(now))),
                    left(secs * 3 / 4)
                        .or(row.resolution_warn_at)
                        .or(Some(at(now))),
                    None,
                )
            } else {
                (None, None, stopped.then_some(elapsed <= secs))
            }
        }
        None => (None, None, None),
    };
    sqlx::query(
        "UPDATE sd_ticket_sla SET response_due_at = $2, response_warn_at = $3, response_met = $4, \
                resolution_elapsed_s = $5, resolution_clock_at = $6, resolution_due_at = $7, \
                resolution_warn_at = $8, resolution_met = $9 \
          WHERE ticket_id = $1",
    )
    .bind(ticket)
    .bind(response_due)
    .bind(response_warn)
    .bind(response_met)
    .bind(elapsed)
    .bind(running.then(|| at(now)))
    .bind(due)
    .bind(warn)
    .bind(met)
    .execute(conn)
    .await?;
    Ok(())
}

/// What happened to a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alert {
    Warning,
    Breached,
}

/// Marks warnings and breaches that are due, records them on each ticket's
/// timeline and emits `ticket.sla_warning` / `ticket.sla_breached`. Runs in
/// system scope (the scheduler); skips organizations where the module is
/// off. Returns how many it marked.
pub async fn sweep(conn: &mut PgConnection) -> DeskResult<usize> {
    // Where the module is off for an organization and those below it.
    const LIVE: &str = "NOT EXISTS (SELECT 1 FROM module_settings ms \
        JOIN organizations mo ON mo.id = ms.org_id JOIN organizations o ON o.id = l.org_id \
        WHERE ms.module_key = 'servicedesk' AND NOT ms.enabled AND o.path <@ mo.path)";
    let checks: [(&str, &str, Alert, &str); 4] = [
        (
            "response",
            "response_warned_at",
            Alert::Warning,
            "l.response_warn_at <= now() AND t.first_response_at IS NULL",
        ),
        (
            "response",
            "response_breached_at",
            Alert::Breached,
            "l.response_due_at <= coalesce(t.first_response_at, now()) \
             AND (t.first_response_at IS NULL OR t.first_response_at > l.response_due_at)",
        ),
        (
            "resolution",
            "resolution_warned_at",
            Alert::Warning,
            "l.resolution_clock_at IS NOT NULL AND l.resolution_warn_at <= now()",
        ),
        (
            "resolution",
            "resolution_breached_at",
            Alert::Breached,
            "(l.resolution_clock_at IS NOT NULL AND l.resolution_due_at <= now()) \
             OR l.resolution_met = false",
        ),
    ];
    let mut marked = 0;
    for (clock, column, alert, condition) in checks {
        let hits: Vec<(Uuid, Uuid, String, String, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "UPDATE sd_ticket_sla l SET {column} = now() FROM sd_tickets t \
              WHERE t.id = l.ticket_id AND l.{column} IS NULL AND ({condition}) AND {LIVE} \
              RETURNING t.id, t.org_id, t.number_prefix || '-' || t.number, t.title, t.priority"
            )))
            .fetch_all(&mut *conn)
            .await?;
        for (id, org_id, reference, title, priority) in hits {
            let (event, kind) = match alert {
                Alert::Warning => ("ticket.sla_warning", "warning"),
                Alert::Breached => ("ticket.sla_breached", "breached"),
            };
            let details = json!({ "clock": clock, "alert": kind });
            sqlx::query(
                "INSERT INTO sd_ticket_activity (id, ticket_id, org_id, kind, details) \
                 VALUES ($1, $2, $3, 'sla', $4)",
            )
            .bind(huxley_core::new_id())
            .bind(id)
            .bind(org_id)
            .bind(&details)
            .execute(&mut *conn)
            .await?;
            outbox::emit(
                conn,
                DomainEvent {
                    event_type: event,
                    org_id: Some(org_id),
                    subject_type: "ticket",
                    subject_id: id,
                    payload: json!({
                        "reference": reference,
                        "title": title,
                        "priority": priority,
                        "clock": clock,
                    }),
                },
            )
            .await?;
            marked += 1;
        }
    }
    Ok(marked)
}

/// How often the sweeper looks for warnings and breaches.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// Sweeps until shutdown while the module is on. Safe to run in several
/// processes: each warning and breach is claimed once by its `UPDATE`.
pub async fn run_sweeper(db: sqlx::PgPool, shutdown: tokio_util::sync::CancellationToken) {
    loop {
        if let Err(e) = sweep_once(&db).await {
            tracing::error!(error = %e, "SLA sweep failed");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(SWEEP_EVERY) => {}
        }
    }
}

async fn sweep_once(db: &sqlx::PgPool) -> DeskResult<()> {
    let mut tx = huxley_db::begin_scoped(db, &huxley_db::OrgScope::System).await?;
    if huxley_modules::is_active(&mut tx, crate::MODULE).await? {
        let marked = sweep(&mut tx).await?;
        if marked > 0 {
            tracing::info!(marked, "marked SLA warnings and breaches");
        }
    }
    tx.commit().await?;
    Ok(())
}
