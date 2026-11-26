//! AI triage (ADR-047): the AI triage pack suggests a ticket's queue and
//! priority from the organization's real options; suggestions are kept to
//! compare with where tickets end up. Accuracy is measured by running the
//! same workflow over a labeled sample set: each answer lands in an
//! evaluation instead of on a ticket.

use std::fmt::Write as _;

use huxley_core::RequestContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{self, TicketChanges};
use crate::{DeskError, DeskResult};

/// The queue answer meaning "none of these".
pub const NO_QUEUE: &str = "(none)";
const PRIORITIES: &[(&str, &str)] = &[
    ("p1", "critical: many people can't work, or an outage"),
    ("p2", "high: someone can't work, or a near deadline"),
    ("p3", "normal: something's wrong, with a workaround"),
    ("p4", "low: a request or a question"),
];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct QueueOption {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}

/// What a ticket at an organization can be triaged to.
#[derive(Debug, Clone, Serialize)]
pub struct Options {
    pub queues: Vec<QueueOption>,
    /// The queue names to choose from, and [`NO_QUEUE`].
    pub queue_names: Vec<String>,
    pub priority_keys: Vec<&'static str>,
    /// A line per queue and priority, for the prompt.
    pub guide: String,
}

/// The queues at `org_id` or above, and the priorities.
pub async fn options(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<Options> {
    let queues: Vec<QueueOption> = sqlx::query_as(
        "SELECT q.id, q.name, q.description FROM sd_queues q JOIN organizations o ON o.id = q.org_id \
          WHERE q.archived_at IS NULL AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY lower(q.name)",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?;
    let mut guide = String::from("Queues:\n");
    for q in &queues {
        let about = q
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let about: String = about.chars().take(300).collect();
        let _ = if about.is_empty() {
            writeln!(guide, "- {}", q.name)
        } else {
            writeln!(guide, "- {}: {about}", q.name)
        };
    }
    let _ = writeln!(guide, "- {NO_QUEUE}: none of these fits\nPriorities:");
    for (key, about) in PRIORITIES {
        let _ = writeln!(guide, "- {key}: {about}");
    }
    let mut queue_names: Vec<String> = queues.iter().map(|q| q.name.clone()).collect();
    queue_names.push(NO_QUEUE.to_owned());
    Ok(Options {
        queues,
        queue_names,
        priority_keys: PRIORITIES.iter().map(|(k, _)| *k).collect(),
        guide,
    })
}

/// A queue by name (or id) among `options`; `Ok(None)` for [`NO_QUEUE`].
fn resolve(options: &Options, answer: &str) -> DeskResult<Option<Uuid>> {
    let answer = answer.trim();
    if answer.is_empty() || answer.eq_ignore_ascii_case(NO_QUEUE) {
        return Ok(None);
    }
    options
        .queues
        .iter()
        .find(|q| q.name.eq_ignore_ascii_case(answer) || q.id.to_string() == answer)
        .map(|q| Some(q.id))
        .ok_or_else(|| DeskError::invalid("queue", format!("{answer:?} isn't one of the queues")))
}

fn priority(answer: &str) -> DeskResult<Option<String>> {
    let p = answer.trim().to_ascii_lowercase();
    if p.is_empty() {
        return Ok(None);
    }
    if PRIORITIES.iter().any(|(k, _)| *k == p) {
        Ok(Some(p))
    } else {
        Err(DeskError::invalid("priority", "p1 to p4"))
    }
}

fn clip(reason: &str) -> Option<String> {
    let r: String = reason.trim().chars().take(300).collect();
    (!r.is_empty()).then_some(r)
}

/// An AI triage answer.
#[derive(Debug, Clone, Copy)]
pub struct Answer<'a> {
    pub queue: &'a str,
    pub priority: &'a str,
    pub reason: &'a str,
    /// The skills it needs, by name.
    pub skills: &'a [String],
}

#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    /// It changed the ticket's queue or priority (no one had worked on it
    /// yet, and the organization lets triage apply).
    pub applied: bool,
    /// The caller may set the ticket's needed skills (the same conditions).
    pub apply_skills: bool,
    /// The organization only takes suggestions (`triage.auto_apply` off).
    pub suggest_only: bool,
    pub queue_id: Option<Uuid>,
    pub priority: Option<String>,
}

/// Records a suggestion for a ticket (with the skills it needs, by name),
/// and applies it while no one has worked on the ticket (no person changed
/// it) unless the organization only takes suggestions: the queue if it has
/// none, and the priority.
pub async fn apply(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket_id: Uuid,
    answer: &Answer<'_>,
) -> DeskResult<Applied> {
    let Answer {
        queue,
        priority: priority_answer,
        reason,
        skills,
    } = *answer;
    let ticket = tickets::get(conn, ticket_id).await?;
    let opts = options(&mut *conn, ticket.org_id).await?;
    let queue_id = resolve(&opts, queue)?;
    let priority = priority(priority_answer)?;
    let touched: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sd_ticket_activity WHERE ticket_id = $1 AND actor_id IS NOT NULL)",
    )
    .bind(ticket_id)
    .fetch_one(&mut *conn)
    .await?;
    let set_queue = queue_id.is_some() && ticket.queue_id.is_none();
    let set_priority = priority.as_ref().is_some_and(|p| *p != ticket.priority);
    let auto = huxley_orgs::settings::value(
        &mut *conn,
        ticket.org_id,
        huxley_orgs::settings::TRIAGE_AUTO,
    )
    .await
    .map_err(|e| DeskError::Sqlx(sqlx::Error::Protocol(e.to_string())))?
    .is_none_or(|v| v == "true");
    let applied = auto && !touched && (set_queue || set_priority);
    if applied {
        let changes = TicketChanges {
            version: ticket.version,
            queue_id: set_queue.then_some(queue_id),
            priority: if set_priority { priority.clone() } else { None },
            ..TicketChanges::default()
        };
        tickets::update(conn, ctx, ticket_id, &changes).await?;
    }
    sqlx::query(
        "INSERT INTO sd_triage (ticket_id, org_id, queue_id, priority, reason, applied, skills) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (ticket_id) DO UPDATE SET queue_id = EXCLUDED.queue_id, priority = EXCLUDED.priority, \
             reason = EXCLUDED.reason, applied = sd_triage.applied OR EXCLUDED.applied, skills = EXCLUDED.skills, at = now()",
    )
    .bind(ticket_id)
    .bind(ticket.org_id)
    .bind(queue_id)
    .bind(&priority)
    .bind(clip(reason))
    .bind(applied)
    .bind(skills)
    .execute(&mut *conn)
    .await?;
    Ok(Applied {
        applied,
        apply_skills: auto && !touched,
        suggest_only: !auto,
        queue_id,
        priority,
    })
}

/// Records an evaluation's answer for a sample.
pub async fn answer(
    conn: &mut PgConnection,
    evaluation_id: Uuid,
    sample_id: Uuid,
    queue: &str,
    priority_answer: &str,
    reason: &str,
) -> DeskResult<()> {
    let org: Option<Uuid> =
        sqlx::query_scalar("SELECT org_id FROM sd_triage_evaluations WHERE id = $1")
            .bind(evaluation_id)
            .fetch_optional(&mut *conn)
            .await?;
    let org = org.ok_or(DeskError::NotFound)?;
    let opts = options(&mut *conn, org).await?;
    let queue_id = resolve(&opts, queue)?;
    let priority = priority(priority_answer)?;
    let done = sqlx::query(
        "UPDATE sd_triage_results SET queue_id = $3, priority = $4, reason = $5, answered_at = now() \
          WHERE evaluation_id = $1 AND sample_id = $2",
    )
    .bind(evaluation_id)
    .bind(sample_id)
    .bind(queue_id)
    .bind(&priority)
    .bind(clip(reason))
    .execute(conn)
    .await?
    .rows_affected();
    if done == 0 {
        return Err(DeskError::NotFound);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TriageSample)]
pub struct Sample {
    pub id: Uuid,
    pub org_id: Uuid,
    pub text: String,
    pub queue_id: Option<Uuid>,
    pub queue_name: Option<String>,
    pub priority: Option<String>,
    pub ticket_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = TriageSampleInput)]
pub struct SampleInput {
    pub text: String,
    #[serde(default)]
    pub queue_id: Option<Uuid>,
    #[serde(default)]
    pub priority: Option<String>,
}

/// The most samples an organization keeps.
pub const MAX_SAMPLES: i64 = 500;

/// An organization's sample set, newest first.
pub async fn samples(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<Vec<Sample>> {
    Ok(sqlx::query_as(
        "SELECT s.id, s.org_id, s.text, s.queue_id, q.name AS queue_name, s.priority, s.ticket_id, s.created_at \
           FROM sd_triage_samples s LEFT JOIN sd_queues q ON q.id = s.queue_id \
          WHERE s.org_id = $1 ORDER BY s.created_at DESC, s.id",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

async fn room(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<i64> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sd_triage_samples WHERE org_id = $1")
        .bind(org_id)
        .fetch_one(conn)
        .await?;
    Ok((MAX_SAMPLES - n).max(0))
}

/// Adds a labeled example by hand.
pub async fn add_sample(
    conn: &mut PgConnection,
    org_id: Uuid,
    input: &SampleInput,
) -> DeskResult<Uuid> {
    let text = input.text.trim();
    if text.is_empty() || text.chars().count() > 8000 {
        return Err(DeskError::invalid("text", "1 to 8000 characters"));
    }
    let priority = priority(input.priority.as_deref().unwrap_or_default())?;
    if let Some(q) = input.queue_id {
        let opts = options(&mut *conn, org_id).await?;
        if !opts.queues.iter().any(|o| o.id == q) {
            return Err(DeskError::invalid("queueId", "not a queue here"));
        }
    }
    if input.queue_id.is_none() && priority.is_none() {
        return Err(DeskError::invalid(
            "queueId",
            "give the right queue, priority, or both",
        ));
    }
    if room(&mut *conn, org_id).await? == 0 {
        return Err(DeskError::invalid(
            "text",
            format!("at most {MAX_SAMPLES} samples"),
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO sd_triage_samples (id, org_id, text, queue_id, priority) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(org_id)
        .bind(text)
        .bind(input.queue_id)
        .bind(&priority)
        .execute(conn)
        .await?;
    Ok(id)
}

/// Adds up to `limit` recently resolved tickets of `org_id` and below that
/// have a queue, as they ended up (people confirmed them by working them).
/// Returns how many were added.
pub async fn add_from_tickets(
    conn: &mut PgConnection,
    org_id: Uuid,
    limit: i64,
) -> DeskResult<u64> {
    let limit = limit.clamp(1, room(&mut *conn, org_id).await?.max(1));
    let added = sqlx::query(
        "INSERT INTO sd_triage_samples (id, org_id, text, queue_id, priority, ticket_id) \
         SELECT gen_random_uuid(), $1, left(t.title || E'\\n\\n' || coalesce(t.description, ''), 8000), t.queue_id, t.priority, t.id \
           FROM sd_tickets t JOIN organizations o ON o.id = t.org_id \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) AND t.resolved_at IS NOT NULL \
            AND t.queue_id IS NOT NULL \
            AND NOT EXISTS (SELECT 1 FROM sd_triage_samples s WHERE s.org_id = $1 AND s.ticket_id = t.id) \
          ORDER BY t.resolved_at DESC LIMIT $2",
    )
    .bind(org_id)
    .bind(limit)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(added)
}

pub async fn delete_sample(conn: &mut PgConnection, id: Uuid) -> DeskResult<bool> {
    Ok(sqlx::query("DELETE FROM sd_triage_samples WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// A started evaluation: each sample to send to the workflow.
#[derive(Debug, Clone)]
pub struct Started {
    pub evaluation_id: Uuid,
    /// `(position, sample id, text)`.
    pub samples: Vec<(i32, Uuid, String)>,
}

/// Starts an evaluation of `org_id`'s sample set (the caller starts a run
/// per sample and calls [`set_run`]).
pub async fn start_evaluation(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    workflow: (Uuid, i32),
    target: i16,
) -> DeskResult<Started> {
    let rows: Vec<(Uuid, String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT id, text, queue_id, priority FROM sd_triage_samples WHERE org_id = $1 ORDER BY created_at, id",
    )
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Err(DeskError::invalid("samples", "add samples first"));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sd_triage_evaluations (id, org_id, workflow_id, workflow_version, target, started_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(workflow.0)
    .bind(workflow.1)
    .bind(target)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let mut samples = Vec::with_capacity(rows.len());
    for (i, (sample, text, queue, priority)) in rows.into_iter().enumerate() {
        let position = i32::try_from(i).unwrap_or(i32::MAX);
        sqlx::query(
            "INSERT INTO sd_triage_results (evaluation_id, sample_id, position, org_id, text, expected_queue_id, expected_priority) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(sample)
        .bind(position)
        .bind(org_id)
        .bind(&text)
        .bind(queue)
        .bind(&priority)
        .execute(&mut *conn)
        .await?;
        samples.push((position, sample, text));
    }
    huxley_db::audit::record(
        &mut *conn,
        ctx,
        huxley_db::audit::AuditRecord::new("triage.evaluation_started")
            .org(org_id)
            .details(json!({ "evaluation": id, "samples": samples.len(), "workflow": workflow.0 })),
    )
    .await?;
    Ok(Started {
        evaluation_id: id,
        samples,
    })
}

/// Notes the run answering a sample.
pub async fn set_run(
    conn: &mut PgConnection,
    evaluation_id: Uuid,
    position: i32,
    run: Uuid,
) -> DeskResult<()> {
    sqlx::query(
        "UPDATE sd_triage_results SET run_id = $3 WHERE evaluation_id = $1 AND position = $2",
    )
    .bind(evaluation_id)
    .bind(position)
    .bind(run)
    .execute(conn)
    .await?;
    Ok(())
}

/// Correct out of how many.
#[derive(Debug, Clone, Copy, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TriageScore)]
pub struct Score {
    pub correct: i64,
    pub of: i64,
}

impl Score {
    /// Percent, once there's something to score.
    #[allow(clippy::cast_precision_loss)]
    pub fn percent(self) -> Option<f64> {
        (self.of > 0).then(|| (self.correct as f64 * 1000.0 / self.of as f64).round() / 10.0)
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TriageMiss)]
pub struct Miss {
    pub position: i32,
    pub text: String,
    pub expected_queue: Option<String>,
    pub expected_priority: Option<String>,
    pub queue: Option<String>,
    pub priority: Option<String>,
    pub reason: Option<String>,
    /// The run failed instead of answering.
    pub failed: bool,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TriageEvaluation)]
pub struct Evaluation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workflow_id: Option<Uuid>,
    pub workflow_version: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    pub total: i64,
    pub answered: i64,
    pub failed: i64,
    /// Still running.
    pub pending: i64,
    /// Samples with a right queue, and how many the workflow got.
    pub queue: Score,
    pub priority: Score,
    /// Queue accuracy in percent (failed runs count as wrong).
    pub accuracy: Option<f64>,
    pub target: i16,
    /// Met the target (once nothing is pending).
    pub passed: Option<bool>,
    pub misses: Vec<Miss>,
}

type Counts = (
    Uuid,
    Option<Uuid>,
    Option<i32>,
    OffsetDateTime,
    i16,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

/// An evaluation with its scores and misses (in the transaction's scope).
pub async fn evaluation(conn: &mut PgConnection, id: Uuid) -> DeskResult<Option<Evaluation>> {
    let row: Option<Counts> = sqlx::query_as(
        "SELECT e.org_id, e.workflow_id, e.workflow_version, e.started_at, e.target, \
                count(r.*), \
                count(r.*) FILTER (WHERE r.answered_at IS NOT NULL), \
                count(r.*) FILTER (WHERE r.answered_at IS NULL AND ru.status IN ('failed', 'cancelled')), \
                count(r.*) FILTER (WHERE r.expected_queue_id IS NOT NULL AND r.queue_id IS NOT DISTINCT FROM r.expected_queue_id AND r.answered_at IS NOT NULL), \
                count(r.*) FILTER (WHERE r.expected_queue_id IS NOT NULL), \
                count(r.*) FILTER (WHERE r.expected_priority IS NOT NULL AND r.priority = r.expected_priority), \
                count(r.*) FILTER (WHERE r.expected_priority IS NOT NULL) \
           FROM sd_triage_evaluations e LEFT JOIN sd_triage_results r ON r.evaluation_id = e.id \
           LEFT JOIN runs ru ON ru.id = r.run_id \
          WHERE e.id = $1 GROUP BY e.id",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        org_id,
        workflow_id,
        workflow_version,
        started_at,
        target,
        total,
        answered,
        failed,
        queue_right,
        queue_total,
        priority_right,
        priority_total,
    )) = row
    else {
        return Ok(None);
    };
    let misses: Vec<Miss> = sqlx::query_as(
        "SELECT r.position, left(r.text, 300) AS text, eq.name AS expected_queue, r.expected_priority, q.name AS queue, \
                r.priority, r.reason, (r.answered_at IS NULL AND coalesce(ru.status IN ('failed', 'cancelled'), false)) AS failed, \
                r.run_id \
           FROM sd_triage_results r LEFT JOIN sd_queues eq ON eq.id = r.expected_queue_id \
           LEFT JOIN sd_queues q ON q.id = r.queue_id LEFT JOIN runs ru ON ru.id = r.run_id \
          WHERE r.evaluation_id = $1 \
            AND ((r.answered_at IS NOT NULL AND ((r.expected_queue_id IS NOT NULL AND r.queue_id IS DISTINCT FROM r.expected_queue_id) \
                                            OR (r.expected_priority IS NOT NULL AND r.priority IS DISTINCT FROM r.expected_priority))) \
                 OR (r.answered_at IS NULL AND ru.status IN ('failed', 'cancelled'))) \
          ORDER BY r.position LIMIT 200",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    let pending = total - answered - failed;
    let queue = Score {
        correct: queue_right,
        of: queue_total,
    };
    let accuracy = queue.percent();
    let passed = (pending == 0).then(|| accuracy.is_some_and(|a| a >= f64::from(target)));
    Ok(Some(Evaluation {
        id,
        org_id,
        workflow_id,
        workflow_version,
        started_at,
        total,
        answered,
        failed,
        pending,
        queue,
        priority: Score {
            correct: priority_right,
            of: priority_total,
        },
        accuracy,
        target,
        passed,
        misses,
    }))
}

/// An organization's evaluations, newest first.
pub async fn evaluations(
    conn: &mut PgConnection,
    org_id: Uuid,
    limit: i64,
) -> DeskResult<Vec<Evaluation>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sd_triage_evaluations WHERE org_id = $1 ORDER BY started_at DESC LIMIT $2",
    )
    .bind(org_id)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(e) = evaluation(&mut *conn, id).await? {
            out.push(e);
        }
    }
    Ok(out)
}

/// How often the AI's suggestions matched where tickets ended up.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TriageAgreement)]
pub struct Agreement {
    /// Resolved tickets with a suggestion, in the last 90 days.
    pub tickets: i64,
    pub queue: Score,
    pub priority: Score,
    /// Suggestions applied to tickets.
    pub applied: i64,
}

/// Agreement on resolved tickets of `org_id` and below in the last 90 days.
pub async fn agreement(conn: &mut PgConnection, org_id: Uuid) -> DeskResult<Agreement> {
    let (tickets, queue_right, queue_total, priority_right, priority_total, applied): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), \
                count(*) FILTER (WHERE tr.queue_id IS NOT NULL AND tr.queue_id = t.queue_id), \
                count(*) FILTER (WHERE tr.queue_id IS NOT NULL), \
                count(*) FILTER (WHERE tr.priority IS NOT NULL AND tr.priority = t.priority), \
                count(*) FILTER (WHERE tr.priority IS NOT NULL), \
                count(*) FILTER (WHERE tr.applied) \
           FROM sd_triage tr JOIN sd_tickets t ON t.id = tr.ticket_id JOIN organizations o ON o.id = t.org_id \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) AND t.resolved_at >= now() - interval '90 days'",
    )
    .bind(org_id)
    .fetch_one(conn)
    .await?;
    Ok(Agreement {
        tickets,
        queue: Score {
            correct: queue_right,
            of: queue_total,
        },
        priority: Score {
            correct: priority_right,
            of: priority_total,
        },
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options {
            queues: vec![QueueOption {
                id: Uuid::nil(),
                name: "Hardware".into(),
                description: String::new(),
            }],
            queue_names: vec!["Hardware".into(), NO_QUEUE.into()],
            priority_keys: vec!["p1", "p2", "p3", "p4"],
            guide: String::new(),
        }
    }

    #[test]
    fn answers_resolve_to_options() {
        assert_eq!(resolve(&opts(), " hardware ").unwrap(), Some(Uuid::nil()));
        assert_eq!(resolve(&opts(), NO_QUEUE).unwrap(), None);
        assert!(resolve(&opts(), "Network").is_err());
        assert_eq!(priority("P2").unwrap().as_deref(), Some("p2"));
        assert!(priority("urgent").is_err());
        assert_eq!(Score { correct: 2, of: 3 }.percent(), Some(66.7));
        assert_eq!(Score::default().percent(), None);
    }
}
