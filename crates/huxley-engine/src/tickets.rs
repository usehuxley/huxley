//! The ticket steps (`core.ticket.create|get|find|update|note`): a run
//! works with the tickets of its organization and those below it, and the
//! `servicedesk` module must be on where it works. Writes refuse values
//! derived from secrets.

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::tickets::{
    self, NewNote, NewTicket, StatusChange, TicketChanges, TicketFilter,
};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

fn desk_error(e: huxley_servicedesk::DeskError) -> Json {
    use huxley_servicedesk::DeskError;
    match e {
        DeskError::NotFound => error("not_found", "not found"),
        DeskError::Invalid { field, message } => error("input", format!("{field}: {message}")),
        DeskError::VersionConflict => error("conflict", "it changed while the step ran; retry it"),
        DeskError::Db(e) => internal(e),
        DeskError::Sqlx(e) => internal(e),
    }
}

/// An optional text input.
fn text(input: &Json, field: &str) -> Result<Option<String>, Json> {
    match &input[field] {
        Json::Null => Ok(None),
        Json::String(s) => Ok(Some(s.clone())),
        Json::Number(n) => Ok(Some(n.to_string())),
        _ => Err(error("input", format!("{field} is text"))),
    }
}

/// An optional id input.
fn id(input: &Json, field: &str) -> Result<Option<Uuid>, Json> {
    text(input, field)?
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse()
                .map_err(|_| error("input", format!("{field} is an id")))
        })
        .transpose()
}

/// An id that's present, missing, or explicitly null (to clear).
#[allow(clippy::option_option)] // JSON's three states: missing, null, a value.
fn id_or_clear(input: &Json, field: &str) -> Result<Option<Option<Uuid>>, Json> {
    match input.get(field) {
        None => Ok(None),
        Some(Json::Null) => Ok(Some(None)),
        Some(_) => Ok(Some(id(input, field)?)),
    }
}

/// An optional 1–3 level.
fn level(input: &Json, field: &str) -> Result<Option<i16>, Json> {
    match &input[field] {
        Json::Null => Ok(None),
        v => v
            .as_i64()
            .and_then(|n| i16::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| error("input", format!("{field} is 1 (high) to 3 (low)"))),
    }
}

fn ticket_id(input: &Json) -> Result<Uuid, Json> {
    id(input, "ticket_id")?.ok_or_else(|| error("input", "ticket_id is needed"))
}

fn context() -> RequestContext {
    RequestContext::system(Via::Workflow)
}

impl Engine {
    /// Runs one of the ticket steps.
    pub(crate) async fn ticket_step(
        &self,
        step_type: &str,
        org_id: Uuid,
        input: &Json,
        tainted: bool,
    ) -> Result<Json, Json> {
        if tainted && step_type != "core.ticket.get" && step_type != "core.ticket.find" {
            return Err(error(
                "input",
                "tickets can't contain secrets or values derived from them",
            ));
        }
        let subtree = self.subtree(org_id).await?;
        // The organization a new ticket is for, and where the module must be on.
        let target = Self::org_in(input, &subtree)?.unwrap_or(org_id);
        match step_type {
            "core.ticket.create" => self.create_ticket(target, &subtree, input).await,
            "core.ticket.find" => {
                self.require_desk(target).await?;
                let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
                    .await
                    .map_err(internal)?;
                let found = find(
                    &mut tx,
                    input,
                    (!input["org_id"].is_null()).then_some(target),
                )
                .await?;
                tx.commit().await.map_err(internal)?;
                Ok(found)
            }
            other => {
                let ticket = ticket_id(input)?;
                let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree.clone()))
                    .await
                    .map_err(internal)?;
                // Invisible outside the run's subtree: not found.
                let found = tickets::get(&mut tx, ticket).await.map_err(desk_error)?;
                tx.commit().await.map_err(internal)?;
                self.require_desk(found.org_id).await?;
                if other == "core.ticket.dispatch" {
                    return self.dispatch(&found, input).await;
                }
                let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree))
                    .await
                    .map_err(internal)?;
                let output = match other {
                    "core.ticket.get" => get(&mut tx, ticket, input).await,
                    "core.ticket.update" => update(&mut tx, ticket, input).await,
                    "core.ticket.note" => note(&mut tx, ticket, input).await,
                    other => Err(error("unsupported", format!("{other} isn't a ticket step"))),
                }?;
                tx.commit().await.map_err(internal)?;
                Ok(output)
            }
        }
    }

    async fn create_ticket(
        &self,
        target: Uuid,
        subtree: &[Uuid],
        input: &Json,
    ) -> Result<Json, Json> {
        self.require_desk(target).await?;
        let new = NewTicket {
            org_id: target,
            r#type: text(input, "type")?,
            title: text(input, "title")?.unwrap_or_default(),
            description: text(input, "description")?,
            impact: level(input, "impact")?,
            urgency: level(input, "urgency")?,
            priority: text(input, "priority")?,
            queue_id: id(input, "queue_id")?,
            requester_id: id(input, "requester_id")?,
            assignee_user_id: id(input, "assignee_user_id")?,
            assignee_team_id: id(input, "assignee_team_id")?,
            source: Some(text(input, "source")?.unwrap_or_else(|| "workflow".into())),
        };
        // The prefix and SLA come from above too: system scope.
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let prefix = tickets::prefix_for(&mut tx, target)
            .await
            .map_err(desk_error)?;
        let plan = huxley_servicedesk::sla::plan_for(&mut tx, target)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        let mut tx = begin_scoped(self.db(), &OrgScope::Orgs(subtree.to_vec()))
            .await
            .map_err(internal)?;
        let ticket = tickets::create(&mut tx, &context(), &new, &prefix, plan.as_ref())
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "ticket": ticket }))
    }
}

async fn find(tx: &mut PgConnection, input: &Json, org: Option<Uuid>) -> Result<Json, Json> {
    let filter = TicketFilter {
        org_id: org,
        queue_id: id(input, "queue_id")?,
        assignee: text(input, "assignee")?,
        state: text(input, "state")?,
        search: text(input, "search")?,
        priority: text(input, "priority")?,
        limit: input["limit"].as_i64(),
        offset: None,
    };
    let page = tickets::list(tx, &filter).await.map_err(desk_error)?;
    Ok(json!({ "tickets": page.items, "total": page.total }))
}

async fn get(tx: &mut PgConnection, ticket: Uuid, input: &Json) -> Result<Json, Json> {
    let found = tickets::get(tx, ticket).await.map_err(desk_error)?;
    let notes = if input["notes"].as_bool().unwrap_or(false) {
        json!(tickets::notes(tx, ticket).await.map_err(desk_error)?)
    } else {
        Json::Null
    };
    Ok(json!({ "ticket": found, "notes": notes }))
}

/// Changes a ticket's fields, then its status: at its current version (a
/// workflow acts on what's there now).
async fn update(tx: &mut PgConnection, ticket: Uuid, input: &Json) -> Result<Json, Json> {
    let ctx = context();
    let current = tickets::get(tx, ticket).await.map_err(desk_error)?;
    let changes = TicketChanges {
        version: current.version,
        title: text(input, "title")?,
        description: text(input, "description")?,
        impact: level(input, "impact")?,
        urgency: level(input, "urgency")?,
        priority: text(input, "priority")?,
        queue_id: id_or_clear(input, "queue_id")?,
        requester_id: id_or_clear(input, "requester_id")?,
        assignee_user_id: id_or_clear(input, "assignee_user_id")?,
        assignee_team_id: id_or_clear(input, "assignee_team_id")?,
    };
    let touches_fields = changes.title.is_some()
        || changes.description.is_some()
        || changes.impact.is_some()
        || changes.urgency.is_some()
        || changes.priority.is_some()
        || changes.queue_id.is_some()
        || changes.requester_id.is_some()
        || changes.assignee_user_id.is_some()
        || changes.assignee_team_id.is_some();
    let mut updated = if touches_fields {
        tickets::update(tx, &ctx, ticket, &changes)
            .await
            .map_err(desk_error)?
    } else {
        current
    };
    if let Some(status) = text(input, "status")?
        && status != updated.status_key
    {
        let change = StatusChange {
            status,
            resolution: text(input, "resolution")?,
            version: updated.version,
        };
        updated = tickets::change_status(tx, &ctx, ticket, &change)
            .await
            .map_err(desk_error)?;
    }
    Ok(json!({ "ticket": updated }))
}

async fn note(tx: &mut PgConnection, ticket: Uuid, input: &Json) -> Result<Json, Json> {
    let note = NewNote {
        kind: text(input, "kind")?.unwrap_or_else(|| "internal".into()),
        body: text(input, "body")?.unwrap_or_default(),
    };
    let added = tickets::add_note(tx, &context(), ticket, &note)
        .await
        .map_err(desk_error)?;
    Ok(json!({ "note": added }))
}
