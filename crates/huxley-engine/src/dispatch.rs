//! `core.ticket.dispatch`: assigns a ticket by its queue's strategy. The
//! queue's members are the candidates; with the `workforce` module on (at
//! the queue's organization), people missing a required certification, away
//! or outside their hours are skipped, and `skills` ranks by how well
//! people's skills meet the requirements. Without it, `skills` falls back
//! to `load`. Dispatch for one queue happens one ticket at a time, so turns
//! stay fair.

use std::collections::BTreeMap;

use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::dispatch::{self, Candidate, STRATEGIES};
use huxley_servicedesk::tickets::{self, Ticket, TicketChanges};
use huxley_workforce::requirements::{self, Fit};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

fn outcome(
    strategy: Option<&str>,
    reason: &str,
    assignee: Option<&Candidate>,
    considered: &[Json],
) -> Json {
    json!({
        "assigned": assignee.is_some(),
        "assignee": assignee.map(|c| json!({ "id": c.user_id, "name": c.name })),
        "strategy": strategy,
        "reason": reason,
        "considered": considered,
    })
}

/// Why a candidate can't take the work, if they can't.
fn skipped(fit: Option<&Fit>) -> Option<Json> {
    let fit = fit?;
    if !fit.qualified {
        return Some(json!({ "reason": "unqualified", "missing": fit.missing }));
    }
    fit.unavailable.as_ref().map(|why| json!({ "reason": why }))
}

/// Candidates in the order the strategy prefers them, and what was
/// considered about each.
async fn rank(
    tx: &mut PgConnection,
    strategy: &str,
    queue: Uuid,
    ticket: Uuid,
    workforce: bool,
) -> Result<(Vec<Candidate>, Vec<Json>), Json> {
    let mut people = dispatch::candidates(tx, queue).await.map_err(internal)?;
    if strategy != "round_robin" {
        dispatch::by_load(&mut people);
    }
    let mut fits: BTreeMap<Uuid, Fit> = BTreeMap::new();
    if workforce && !people.is_empty() {
        let type_id: Uuid = sqlx::query_scalar("SELECT type_id FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
        let ids: Vec<Uuid> = people.iter().map(|c| c.user_id).collect();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let work = requirements::Work {
            queue: Some(queue),
            ticket_type: Some(type_id),
            ticket: Some(ticket),
        };
        for fit in requirements::fit(tx, work, Some(&ids), Some(now))
            .await
            .map_err(internal)?
        {
            fits.insert(fit.user_id, fit);
        }
        if strategy == "skills" {
            // Stable: equal matches keep the load, then round-robin, order.
            let score = |c: &Candidate| fits.get(&c.user_id).map_or(0.0, |f| f.skill_match);
            people.sort_by(|a, b| score(b).total_cmp(&score(a)));
        }
    }
    let considered = people
        .iter()
        .map(|c| {
            json!({
                "id": c.user_id,
                "name": c.name,
                "open_tickets": c.open_tickets,
                "skill_match": fits.get(&c.user_id).map(|f| f.skill_match),
                "skipped": skipped(fits.get(&c.user_id)),
            })
        })
        .collect();
    let eligible = people
        .into_iter()
        .filter(|c| skipped(fits.get(&c.user_id)).is_none())
        .collect();
    Ok((eligible, considered))
}

impl Engine {
    /// `core.ticket.dispatch` for a ticket the run can see.
    pub(crate) async fn dispatch(&self, ticket: &Ticket, input: &Json) -> Result<Json, Json> {
        let reassign = match &input["reassign"] {
            Json::Null => false,
            Json::Bool(b) => *b,
            _ => return Err(error("input", "reassign is true or false")),
        };
        let chosen_strategy = match &input["strategy"] {
            Json::Null => None,
            Json::String(s) if STRATEGIES.contains(&s.as_str()) => Some(s.clone()),
            _ => {
                return Err(error(
                    "input",
                    "strategy is manual, round_robin, load or skills",
                ));
            }
        };
        if ticket.assignee_user_id.is_some() && !reassign {
            return Ok(outcome(None, "assigned", None, &[]));
        }
        let Some(queue) = ticket.queue_id else {
            return Ok(outcome(None, "no_queue", None, &[]));
        };
        // Queues usually belong to the MSP, above the ticket: system scope.
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        // One at a time per queue, so turns and loads are current.
        let queue_org: Uuid =
            sqlx::query_scalar("SELECT org_id FROM sd_queues WHERE id = $1 FOR UPDATE")
                .bind(queue)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
        let strategy = match chosen_strategy {
            Some(s) => s,
            None => dispatch::strategy(&mut tx, queue)
                .await
                .map_err(|e| internal(e.to_string()))?,
        };
        if strategy == "manual" {
            tx.commit().await.map_err(internal)?;
            return Ok(outcome(Some(&strategy), "manual", None, &[]));
        }
        let workforce = huxley_modules::is_active_at(&mut tx, huxley_workforce::MODULE, queue_org)
            .await
            .map_err(internal)?;
        let (eligible, considered) = rank(&mut tx, &strategy, queue, ticket.id, workforce).await?;
        let Some(choice) = eligible.first() else {
            tx.commit().await.map_err(internal)?;
            return Ok(outcome(
                Some(&strategy),
                "nobody_available",
                None,
                &considered,
            ));
        };
        // Current, in case it changed since the run looked.
        let current = tickets::get(&mut tx, ticket.id)
            .await
            .map_err(|e| internal(e.to_string()))?;
        if current.assignee_user_id.is_some() && !reassign {
            tx.commit().await.map_err(internal)?;
            return Ok(outcome(Some(&strategy), "assigned", None, &considered));
        }
        let changes = TicketChanges {
            version: current.version,
            assignee_user_id: Some(Some(choice.user_id)),
            ..TicketChanges::default()
        };
        let ctx = huxley_core::RequestContext::system(huxley_core::Via::Workflow);
        tickets::update(&mut tx, &ctx, ticket.id, &changes)
            .await
            .map_err(|e| match e {
                huxley_servicedesk::DeskError::VersionConflict => error(
                    "conflict",
                    "the ticket changed while it was dispatched; retry it",
                ),
                e => internal(e.to_string()),
            })?;
        dispatch::took_turn(&mut tx, queue, choice.user_id)
            .await
            .map_err(|e| internal(e.to_string()))?;
        tx.commit().await.map_err(internal)?;
        Ok(outcome(
            Some(&strategy),
            "dispatched",
            Some(choice),
            &considered,
        ))
    }
}

impl Engine {
    /// `core.dispatch.recommend`: who should take a ticket (in the run's
    /// organization or below), best first, with reasons. Output: `{
    /// recommendations, top }`.
    pub(crate) async fn recommend_assignees(
        &self,
        org_id: Uuid,
        input: &Json,
    ) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let ticket: Uuid = input["ticket_id"]
            .as_str()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| error("input", "ticket_id is required"))?;
        let limit = input["limit"]
            .as_u64()
            .map_or(5, |n| usize::try_from(n).unwrap_or(5))
            .clamp(1, 20);
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let at: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
            .bind(ticket)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal)?;
        let at = at.ok_or_else(|| error("not_found", "No such ticket here"))?;
        tx.commit().await.map_err(internal)?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let workforce = huxley_modules::is_active_at(&mut tx, huxley_workforce::MODULE, at)
            .await
            .map_err(internal)?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let all = huxley_workforce::recommend::for_ticket(&mut tx, ticket, now, workforce, limit)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "recommendations": all, "top": all.first() }))
    }
}
