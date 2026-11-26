//! Change management (ADR-054): a change ticket's type, risk, plans,
//! window and affected assets, through submit → decide → implement →
//! review; freeze windows; the change calendar. Deciding is the Change
//! approvals pack's (CAB, then client approvers for high risk), through
//! `core.change.decide`.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult};

/// The risk questionnaire: key, question, and options (key, label, points).
/// A risk question: key, question, and options (key, label, points).
pub type Question = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str, i16)],
);

pub const QUESTIONS: &[Question] = &[
    (
        "reach",
        "Who would notice if it went wrong?",
        &[
            ("nobody", "Nobody", 0),
            ("few", "A few people", 1),
            ("many", "Many people", 2),
            ("everyone", "Everyone", 3),
        ],
    ),
    (
        "downtime",
        "How much downtime does it need?",
        &[
            ("none", "None", 0),
            ("minutes", "Minutes", 1),
            ("hours", "Hours", 3),
        ],
    ),
    (
        "tested",
        "Has it been tested?",
        &[
            ("yes", "Yes, fully", 0),
            ("partly", "Partly", 1),
            ("no", "No", 2),
        ],
    ),
    (
        "backout",
        "How hard is it to back out?",
        &[
            ("easy", "Easy", 0),
            ("hard", "Hard", 2),
            ("impossible", "It can't be", 3),
        ],
    ),
    (
        "familiar",
        "Have you done this before?",
        &[
            ("often", "Often", 0),
            ("once", "Once or twice", 1),
            ("never", "Never", 2),
        ],
    ),
];

/// Scores at or under this are low risk; at or under [`MEDIUM_MAX`], medium.
pub const LOW_MAX: i16 = 3;
pub const MEDIUM_MAX: i16 = 7;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RiskOption {
    pub key: &'static str,
    pub label: &'static str,
    pub points: i16,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RiskQuestion {
    pub key: &'static str,
    pub question: &'static str,
    pub options: Vec<RiskOption>,
}

/// The questionnaire, for a form.
pub fn questions() -> Vec<RiskQuestion> {
    QUESTIONS
        .iter()
        .map(|(key, question, options)| RiskQuestion {
            key,
            question,
            options: options
                .iter()
                .map(|(key, label, points)| RiskOption {
                    key,
                    label,
                    points: *points,
                })
                .collect(),
        })
        .collect()
}

/// The score and level of complete answers (`None`: some unanswered).
pub fn risk(answers: &Json) -> DeskResult<Option<(i16, &'static str)>> {
    let mut score = 0;
    for (key, _, options) in QUESTIONS {
        match answers[*key].as_str() {
            None => return Ok(None),
            Some(chosen) => {
                let (_, _, points) =
                    options.iter().find(|(k, ..)| *k == chosen).ok_or_else(|| {
                        DeskError::invalid(&format!("riskAnswers/{key}"), "not one of the options")
                    })?;
                score += points;
            }
        }
    }
    let level = if score <= LOW_MAX {
        "low"
    } else if score <= MEDIUM_MAX {
        "medium"
    } else {
        "high"
    };
    Ok(Some((score, level)))
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChangeRecord)]
pub struct Change {
    pub ticket_id: Uuid,
    pub org_id: Uuid,
    /// `standard` (pre-approved), `normal` or `emergency`.
    pub change_type: String,
    #[schema(value_type = std::collections::BTreeMap<String, String>)]
    pub risk_answers: Json,
    pub risk_score: Option<i16>,
    /// `low`, `medium` or `high`, once every question is answered.
    pub risk_level: Option<String>,
    pub implementation_plan: String,
    pub backout_plan: String,
    pub test_plan: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub planned_start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub planned_end: Option<OffsetDateTime>,
    pub affected_assets: Vec<Uuid>,
    /// `draft`, `submitted`, `approved`, `rejected`, `implemented`,
    /// `reviewed` or `cancelled`.
    pub state: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
    pub decided_by: Option<String>,
    pub decision_comment: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub implemented_at: Option<OffsetDateTime>,
    pub outcome: Option<String>,
    pub review: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<OffsetDateTime>,
    pub version: i32,
}

const COLUMNS: &str = "ticket_id, org_id, change_type, risk_answers, risk_score, risk_level, implementation_plan, \
     backout_plan, test_plan, planned_start, planned_end, affected_assets, state, submitted_at, decided_at, decided_by, \
     decision_comment, implemented_at, outcome, review, reviewed_at, version";

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeInput {
    #[serde(default)]
    pub change_type: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::BTreeMap<String, String>>)]
    pub risk_answers: Option<Json>,
    #[serde(default)]
    pub implementation_plan: Option<String>,
    #[serde(default)]
    pub backout_plan: Option<String>,
    #[serde(default)]
    pub test_plan: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub planned_start: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub planned_end: Option<OffsetDateTime>,
    #[serde(default)]
    pub affected_assets: Option<Vec<Uuid>>,
    /// The version you read (after the first save).
    #[serde(default)]
    pub version: Option<i32>,
}

/// A change ticket's org, checking it's a change.
async fn change_ticket(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Uuid> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT t.org_id, ty.key FROM sd_tickets t JOIN sd_ticket_types ty ON ty.id = t.type_id WHERE t.id = $1",
    )
    .bind(ticket)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        None => Err(DeskError::NotFound),
        Some((_, kind)) if kind != "change" => {
            Err(DeskError::invalid("ticket", "not a change ticket"))
        }
        Some((org, _)) => Ok(org),
    }
}

/// A change ticket's change details (a draft with nothing filled in, at
/// first).
pub async fn get(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Change> {
    let org = change_ticket(&mut *conn, ticket).await?;
    sqlx::query(
        "INSERT INTO sd_changes (ticket_id, org_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(ticket)
    .bind(org)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM sd_changes WHERE ticket_id = $1"
    )))
    .bind(ticket)
    .fetch_one(conn)
    .await?)
}

fn text(value: Option<&String>, current: &str, field: &str) -> DeskResult<String> {
    let v = value.map_or_else(|| current.to_owned(), |s| s.trim().to_owned());
    if v.chars().count() > 20_000 {
        return Err(DeskError::invalid(field, "at most 20,000 characters"));
    }
    Ok(v)
}

/// Changes a draft (or rejected) change.
pub async fn save(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    input: &ChangeInput,
) -> DeskResult<Change> {
    let current = get(&mut *conn, ticket).await?;
    if !matches!(current.state.as_str(), "draft" | "rejected") {
        return Err(DeskError::invalid(
            "state",
            "only a draft or rejected change can be changed",
        ));
    }
    if input.version.is_some_and(|v| v != current.version) {
        return Err(DeskError::VersionConflict);
    }
    let change_type = input
        .change_type
        .clone()
        .unwrap_or(current.change_type.clone());
    if !["standard", "normal", "emergency"].contains(&change_type.as_str()) {
        return Err(DeskError::invalid(
            "changeType",
            "standard, normal or emergency",
        ));
    }
    let answers = input
        .risk_answers
        .clone()
        .unwrap_or(current.risk_answers.clone());
    let scored = risk(&answers)?;
    let start = input.planned_start.or(current.planned_start);
    let end = input.planned_end.or(current.planned_end);
    if let (Some(s), Some(e)) = (start, end)
        && e <= s
    {
        return Err(DeskError::invalid("plannedEnd", "after the start"));
    }
    let assets = input
        .affected_assets
        .clone()
        .unwrap_or(current.affected_assets.clone());
    let saved: Change = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE sd_changes SET change_type = $2, risk_answers = $3, risk_score = $4, risk_level = $5, \
                implementation_plan = $6, backout_plan = $7, test_plan = $8, planned_start = $9, planned_end = $10, \
                affected_assets = $11, state = 'draft', version = version + 1 \
          WHERE ticket_id = $1 RETURNING {COLUMNS}"
    )))
    .bind(ticket)
    .bind(&change_type)
    .bind(&answers)
    .bind(scored.map(|s| s.0))
    .bind(scored.map(|s| s.1))
    .bind(text(input.implementation_plan.as_ref(), &current.implementation_plan, "implementationPlan")?)
    .bind(text(input.backout_plan.as_ref(), &current.backout_plan, "backoutPlan")?)
    .bind(text(input.test_plan.as_ref(), &current.test_plan, "testPlan")?)
    .bind(start)
    .bind(end)
    .bind(&assets)
    .fetch_one(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("change.updated")
            .org(saved.org_id)
            .target("ticket", ticket),
    )
    .await?;
    Ok(saved)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChangeFreeze)]
pub struct Freeze {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
}

const FREEZES: &str = "SELECT f.id, f.org_id, o.name AS org_name, f.name, f.starts_at, f.ends_at \
     FROM sd_change_freezes f JOIN organizations o ON o.id = f.org_id";

/// Freezes covering `org_id` (set there or above) that overlap the window.
pub async fn freezes_hitting(
    conn: &mut PgConnection,
    org_id: Uuid,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> DeskResult<Vec<Freeze>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{FREEZES} WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
            AND f.starts_at < $3 AND f.ends_at > $2 ORDER BY f.starts_at"
    )))
    .bind(org_id)
    .bind(start)
    .bind(end)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChangeConflict)]
pub struct Conflict {
    pub ticket_id: Uuid,
    pub reference: String,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub planned_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub planned_end: OffsetDateTime,
}

/// Other submitted or approved changes in an overlapping window touching
/// any of the same assets.
pub async fn conflicts(
    conn: &mut PgConnection,
    ticket: Uuid,
    start: OffsetDateTime,
    end: OffsetDateTime,
    assets: &[Uuid],
) -> DeskResult<Vec<Conflict>> {
    if assets.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT c.ticket_id, t.number_prefix || '-' || t.number AS reference, t.title, c.planned_start, c.planned_end \
           FROM sd_changes c JOIN sd_tickets t ON t.id = c.ticket_id \
          WHERE c.ticket_id <> $1 AND c.state IN ('submitted', 'approved') AND c.planned_start < $3 AND c.planned_end > $2 \
            AND c.affected_assets && $4 ORDER BY c.planned_start",
    )
    .bind(ticket)
    .bind(start)
    .bind(end)
    .bind(assets)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChangeSubmitted)]
pub struct Submitted {
    pub change: Change,
    /// Other changes touching the same assets at the same time.
    pub conflicts: Vec<Conflict>,
}

async fn emit(
    conn: &mut PgConnection,
    event: &'static str,
    c: &Change,
    reference: &str,
    extra: Json,
) -> DeskResult<()> {
    let mut payload = json!({
        "reference": reference,
        "changeType": c.change_type,
        "riskLevel": c.risk_level,
        "riskScore": c.risk_score,
        "plannedStart": c.planned_start.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        "plannedEnd": c.planned_end.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        "affectedAssets": c.affected_assets.len(),
        "state": c.state,
    });
    if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(c.org_id),
            subject_type: "ticket",
            subject_id: c.ticket_id,
            payload,
        },
    )
    .await?;
    Ok(())
}

async fn reference(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<(String, String)> {
    Ok(
        sqlx::query_as(
            "SELECT number_prefix || '-' || number, title FROM sd_tickets WHERE id = $1",
        )
        .bind(ticket)
        .fetch_one(conn)
        .await?,
    )
}

async fn set_state(
    conn: &mut PgConnection,
    ticket: Uuid,
    sql: &str,
    from: &[&str],
) -> DeskResult<Change> {
    sqlx::query_as::<_, Change>(sqlx::AssertSqlSafe(format!(
        "UPDATE sd_changes SET {sql}, version = version + 1 WHERE ticket_id = $1 AND state = ANY($2) RETURNING {COLUMNS}"
    )))
    .bind(ticket)
    .bind(from)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| DeskError::invalid("state", format!("only a change that's {} can do that", from.join(" or "))))
}

/// Submits a change for approval: it needs every risk question answered, a
/// window, and (unless standard) implementation and backout plans; outside
/// emergencies, its window can't fall in a freeze. Standard changes are
/// approved at once. Raises `change.submitted` (or `change.approved`).
pub async fn submit(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
) -> DeskResult<Submitted> {
    let c = get(&mut *conn, ticket).await?;
    if !matches!(c.state.as_str(), "draft" | "rejected") {
        return Err(DeskError::invalid("state", "it's been submitted already"));
    }
    if c.risk_level.is_none() {
        return Err(DeskError::invalid(
            "riskAnswers",
            "answer every risk question",
        ));
    }
    let (Some(start), Some(end)) = (c.planned_start, c.planned_end) else {
        return Err(DeskError::invalid(
            "plannedStart",
            "give the window it's planned for",
        ));
    };
    if c.change_type != "standard"
        && (c.implementation_plan.is_empty() || c.backout_plan.is_empty())
    {
        return Err(DeskError::invalid(
            "implementationPlan",
            "write the implementation and backout plans",
        ));
    }
    if c.change_type != "emergency" {
        let frozen = freezes_hitting(&mut *conn, c.org_id, start, end).await?;
        if let Some(f) = frozen.first() {
            return Err(DeskError::invalid(
                "plannedStart",
                format!(
                    "the window falls in the freeze “{}” ({}); move it, or make it an emergency change",
                    f.name, f.org_name
                ),
            ));
        }
    }
    let conflicts = conflicts(&mut *conn, ticket, start, end, &c.affected_assets).await?;
    let (event, sql) = if c.change_type == "standard" {
        (
            "change.approved",
            "state = 'approved', submitted_at = now(), decided_at = now(), decided_by = 'Pre-approved (standard change)'",
        )
    } else {
        (
            "change.submitted",
            "state = 'submitted', submitted_at = now(), decided_at = NULL, decided_by = NULL, decision_comment = NULL",
        )
    };
    let change = set_state(&mut *conn, ticket, sql, &["draft", "rejected"]).await?;
    let (reference, title) = reference(&mut *conn, ticket).await?;
    emit(
        &mut *conn,
        event,
        &change,
        &reference,
        json!({ "title": title, "conflicts": conflicts.len() }),
    )
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(change.org_id)
            .target("ticket", ticket),
    )
    .await?;
    Ok(Submitted { change, conflicts })
}

/// Records the approval decision on a submitted change (the Change
/// approvals pack). Raises `change.approved` or `change.rejected`.
pub async fn decide(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    approved: bool,
    by: &str,
    comment: Option<&str>,
) -> DeskResult<Change> {
    change_ticket(&mut *conn, ticket).await?;
    let state = if approved { "approved" } else { "rejected" };
    let change: Change = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE sd_changes SET state = $3, decided_at = now(), decided_by = $4, decision_comment = $5, version = version + 1 \
          WHERE ticket_id = $1 AND state = $2 RETURNING {COLUMNS}"
    )))
    .bind(ticket)
    .bind("submitted")
    .bind(state)
    .bind(by.trim().chars().take(200).collect::<String>())
    .bind(comment.map(|c| c.trim().chars().take(2000).collect::<String>()).filter(|c| !c.is_empty()))
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DeskError::invalid("state", "only a submitted change can be decided"))?;
    let (reference, title) = reference(&mut *conn, ticket).await?;
    let event = if approved {
        "change.approved"
    } else {
        "change.rejected"
    };
    emit(&mut *conn, event, &change, &reference, json!({ "title": title, "decidedBy": change.decided_by, "comment": change.decision_comment })).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(event)
            .org(change.org_id)
            .target("ticket", ticket),
    )
    .await?;
    Ok(change)
}

/// Records that an approved change was carried out, and how it went.
pub async fn implemented(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    outcome: &str,
) -> DeskResult<Change> {
    if !["successful", "failed", "backed_out"].contains(&outcome) {
        return Err(DeskError::invalid(
            "outcome",
            "successful, failed or backed_out",
        ));
    }
    let change = sqlx::query_as::<_, Change>(sqlx::AssertSqlSafe(format!(
        "UPDATE sd_changes SET state = 'implemented', implemented_at = now(), outcome = $2, version = version + 1 \
          WHERE ticket_id = $1 AND state = 'approved' RETURNING {COLUMNS}"
    )))
    .bind(ticket)
    .bind(outcome)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DeskError::invalid("state", "only an approved change can be implemented"))?;
    let (reference, title) = reference(&mut *conn, ticket).await?;
    emit(
        &mut *conn,
        "change.implemented",
        &change,
        &reference,
        json!({ "title": title, "outcome": outcome }),
    )
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("change.implemented")
            .org(change.org_id)
            .target("ticket", ticket),
    )
    .await?;
    Ok(change)
}

/// The post-implementation review.
pub async fn review(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    text: &str,
) -> DeskResult<Change> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 20_000 {
        return Err(DeskError::invalid("review", "1 to 20,000 characters"));
    }
    let change = sqlx::query_as::<_, Change>(sqlx::AssertSqlSafe(format!(
        "UPDATE sd_changes SET state = 'reviewed', reviewed_at = now(), review = $2, version = version + 1 \
          WHERE ticket_id = $1 AND state = 'implemented' RETURNING {COLUMNS}"
    )))
    .bind(ticket)
    .bind(text)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DeskError::invalid("state", "review a change once it's implemented"))?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("change.reviewed")
            .org(change.org_id)
            .target("ticket", ticket),
    )
    .await?;
    Ok(change)
}

/// Freezes in the transaction's scope ending after `from`, soonest first.
pub async fn freezes(conn: &mut PgConnection, from: OffsetDateTime) -> DeskResult<Vec<Freeze>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{FREEZES} WHERE f.ends_at > $1 ORDER BY f.starts_at"
    )))
    .bind(from)
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ChangeFreezeInput)]
pub struct FreezeInput {
    pub org_id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
}

pub async fn add_freeze(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &FreezeInput,
) -> DeskResult<Uuid> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(DeskError::invalid("name", "1 to 200 characters"));
    }
    if input.ends_at <= input.starts_at {
        return Err(DeskError::invalid("endsAt", "after the start"));
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO sd_change_freezes (id, org_id, name, starts_at, ends_at) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(input.org_id)
        .bind(name)
        .bind(input.starts_at)
        .bind(input.ends_at)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("change.freeze_added")
            .org(input.org_id)
            .details(json!({ "name": name })),
    )
    .await?;
    Ok(id)
}

pub async fn delete_freeze(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> DeskResult<bool> {
    let org: Option<Uuid> =
        sqlx::query_scalar("DELETE FROM sd_change_freezes WHERE id = $1 RETURNING org_id")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(org) = org {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("change.freeze_removed").org(org),
        )
        .await?;
    }
    Ok(org.is_some())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CalendarChange {
    pub ticket_id: Uuid,
    pub reference: String,
    pub title: String,
    pub org_name: String,
    pub change_type: String,
    pub risk_level: Option<String>,
    pub state: String,
    #[serde(with = "time::serde::rfc3339")]
    pub planned_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub planned_end: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChangeCalendar)]
pub struct Calendar {
    pub changes: Vec<CalendarChange>,
    pub freezes: Vec<Freeze>,
}

/// Changes planned (submitted, approved or implemented) and freezes
/// overlapping `from`..`to`, in the transaction's scope.
pub async fn calendar(
    conn: &mut PgConnection,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> DeskResult<Calendar> {
    let changes = sqlx::query_as(
        "SELECT c.ticket_id, t.number_prefix || '-' || t.number AS reference, t.title, o.name AS org_name, c.change_type, \
                c.risk_level, c.state, c.planned_start, c.planned_end \
           FROM sd_changes c JOIN sd_tickets t ON t.id = c.ticket_id JOIN organizations o ON o.id = c.org_id \
          WHERE c.state IN ('submitted', 'approved', 'implemented') AND c.planned_start < $2 AND c.planned_end > $1 \
          ORDER BY c.planned_start",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *conn)
    .await?;
    let freezes = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{FREEZES} WHERE f.starts_at < $2 AND f.ends_at > $1 ORDER BY f.starts_at"
    )))
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?;
    Ok(Calendar { changes, freezes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_score_into_levels() {
        let low = json!({ "reach": "nobody", "downtime": "none", "tested": "yes", "backout": "easy", "familiar": "often" });
        assert_eq!(risk(&low).unwrap(), Some((0, "low")));
        let high = json!({ "reach": "everyone", "downtime": "hours", "tested": "no", "backout": "hard", "familiar": "never" });
        assert_eq!(risk(&high).unwrap(), Some((12, "high")));
        assert_eq!(risk(&json!({ "reach": "few" })).unwrap(), None);
        assert!(risk(&json!({ "reach": "aliens" })).is_err());
        assert_eq!(questions().len(), 5);
    }
}
