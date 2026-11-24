//! The `servicedesk` module's steps (`core.ticket.*`). Like the record
//! steps, a run works with its own organization and those below it, and
//! each step needs the module active there.

use huxley_core::{RequestContext, Via};
use huxley_db::{OrgScope, begin_scoped};
use huxley_servicedesk::DeskError;
use huxley_servicedesk::mail;
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn desk_error(e: DeskError) -> Json {
    match e {
        DeskError::NotFound => error("not_found", "not found"),
        DeskError::Invalid { field, message } => error("input", format!("{field}: {message}")),
        DeskError::VersionConflict => error("conflict", "it changed while the step ran; retry it"),
        DeskError::Db(e) => error("internal", e.to_string()),
        DeskError::Sqlx(e) => error("internal", e.to_string()),
    }
}

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

impl Engine {
    /// Fails unless the `servicedesk` module is active at `org_id`.
    async fn require_desk(&self, org_id: Uuid) -> Result<(), Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let active = huxley_modules::is_active_at(&mut tx, huxley_servicedesk::MODULE, org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if active {
            Ok(())
        } else {
            Err(error(
                "module_disabled",
                "The service desk (the servicedesk module) is turned off for this organization",
            ))
        }
    }

    /// `core.ticket.from_email`: a received email to ticket work, with its
    /// attachments on the ticket.
    pub(crate) async fn ticket_from_email(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let email_id = input["email_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(|| error("input", "email_id is an email's id"))?;
        let ctx = RequestContext::system(Via::Workflow);
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let outcome = mail::to_ticket(&mut tx, &ctx, email_id, org_id)
            .await
            .map_err(desk_error)?;
        let mut attached = 0;
        if outcome.fresh
            && let Some(ticket) = outcome.ticket_id
            && let Some(storage) = self.payloads.storage()
        {
            let email = mail::email(&mut tx, email_id).await.map_err(desk_error)?;
            let raw = storage.get(&email.raw_key).await.map_err(internal)?;
            let parsed = huxley_mail::parse(&raw)
                .ok_or_else(|| internal("the stored message can't be read"))?;
            for file in parsed.attachments.into_iter().filter(|a| !a.inline) {
                let upload = huxley_entity::attachments::Upload {
                    entity_type: huxley_entity::TICKET,
                    entity_id: ticket,
                    filename: &file.filename,
                    content_type: Some(&file.content_type),
                    data: bytes::Bytes::from(file.data),
                };
                // A file too big for storage is left out, not the message.
                match huxley_entity::attachments::upload(&mut tx, storage, &ctx, upload).await {
                    Ok(_) => attached += 1,
                    Err(huxley_entity::EntityError::TooLarge(_)) => {}
                    Err(e) => return Err(internal(e)),
                }
            }
        }
        tx.commit().await.map_err(internal)?;
        Ok(json!({
            "outcome": outcome.outcome,
            "detail": outcome.detail,
            "ticket_id": outcome.ticket_id,
            "created": outcome.created,
            "attachments": attached,
        }))
    }
}
