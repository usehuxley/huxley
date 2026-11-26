//! The AI triage steps (ADR-047): `core.triage.options` lists what a ticket
//! can be triaged to, and `core.triage.apply` records the AI's answer on a
//! ticket, or in an evaluation for a labeled sample.

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::{DeskError, triage};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

fn desk_error(e: DeskError) -> Json {
    match e {
        DeskError::NotFound => error("not_found", "not found here"),
        DeskError::Invalid { field, message } => error("input", format!("{field}: {message}")),
        DeskError::VersionConflict => error("conflict", "it changed while the step ran; retry it"),
        DeskError::Db(e) => internal(e),
        DeskError::Sqlx(e) => internal(e),
    }
}

/// An optional id input (empty: none).
fn id(input: &Json, key: &str) -> Result<Option<Uuid>, Json> {
    match input[key].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => s
            .parse()
            .map(Some)
            .map_err(|_| error("input", format!("{key} isn't an id"))),
    }
}

fn text<'a>(input: &'a Json, key: &str) -> &'a str {
    input[key].as_str().unwrap_or_default()
}

impl Engine {
    /// `core.triage.options`: `{ queues, queue_names, priority_keys, guide }`
    /// for a ticket's organization (or the run's).
    pub(crate) async fn triage_options(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let subtree = self.subtree(org_id).await?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let at = match id(input, "ticket_id")? {
            Some(ticket) => sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
                .bind(ticket)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?
                .ok_or_else(|| error("not_found", "No such ticket here"))?,
            None => org_id,
        };
        let options = triage::options(&mut tx, at).await.map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!(options))
    }

    /// `core.triage.apply`: with `ticket_id`, records the suggestion and
    /// applies it while no one has worked the ticket; with `evaluation_id`
    /// and `sample_id`, records the answer in the evaluation. Output: `{
    /// mode, applied, queue_id, priority }`.
    pub(crate) async fn triage_apply(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let subtree = self.subtree(org_id).await?;
        let (queue, priority, reason) = (
            text(input, "queue"),
            text(input, "priority"),
            text(input, "reason"),
        );
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
            .await
            .map_err(internal)?;
        let out = if let Some(ticket) = id(input, "ticket_id")? {
            let ctx = RequestContext::system(Via::Workflow);
            let applied = triage::apply(&mut tx, &ctx, ticket, queue, priority, reason)
                .await
                .map_err(desk_error)?;
            json!({ "mode": "ticket", "applied": applied.applied, "queue_id": applied.queue_id, "priority": applied.priority })
        } else {
            let (Some(evaluation), Some(sample)) =
                (id(input, "evaluation_id")?, id(input, "sample_id")?)
            else {
                return Err(error(
                    "input",
                    "give ticket_id, or evaluation_id and sample_id",
                ));
            };
            triage::answer(&mut tx, evaluation, sample, queue, priority, reason)
                .await
                .map_err(desk_error)?;
            json!({ "mode": "evaluation", "applied": false, "queue_id": null, "priority": priority })
        };
        tx.commit().await.map_err(internal)?;
        Ok(out)
    }
}
