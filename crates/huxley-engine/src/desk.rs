//! The `servicedesk` module's steps (`core.ticket.*`). Like the record
//! steps, a run works with its own organization and those below it, and
//! each step needs the module active there.

use std::fmt::Write as _;

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

    /// Runs a service desk step (`core.ticket.*`, `core.mail.*`).
    pub(crate) async fn desk_step(
        &self,
        kind: &str,
        org_id: Uuid,
        input: &Json,
    ) -> Result<Json, Json> {
        match kind {
            "core.mail.fetch" => self.fetch_mail(org_id, input).await,
            "core.ticket.from_email" => self.ticket_from_email(org_id, input).await,
            "core.ticket.email_reply" => self.email_reply(org_id, input).await,
            other => Err(internal(format!("not a service desk step: {other}"))),
        }
    }

    /// `core.ticket.email_reply`: emails a public note to the ticket's
    /// requester from its mailbox, threaded so their answer comes back to
    /// the ticket. A note that shouldn't be emailed (internal, from the
    /// requester, already sent) is skipped, not failed.
    async fn email_reply(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let note_id = input["note_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(|| error("input", "note_id is a note's id"))?;
        let Some(mailer) = self.mailer.as_ref().filter(|m| m.enabled()) else {
            return Err(error(
                "email",
                "email isn't set up for this instance (email.smtp_url)",
            ));
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let planned = mail::reply_for(&mut tx, note_id, org_id)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        let reply = match planned {
            Ok(reply) => reply,
            Err(reason) => return Ok(json!({ "sent": false, "reason": reason })),
        };
        let mut paragraphs = String::new();
        for p in reply.text.split("\n\n") {
            let _ = write!(
                paragraphs,
                "<p>{}</p>",
                huxley_email::escape_html(p).replace('\n', "<br>")
            );
        }
        let html = format!(
            "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;line-height:1.5\">{paragraphs}</body></html>"
        );
        let email = huxley_email::Email {
            to: reply.to.clone(),
            subject: reply.subject.clone(),
            text: reply.text.clone(),
            html,
            reply: Some(huxley_email::Reply {
                from_name: reply.from_name.clone(),
                from_address: reply.from_address.clone(),
                message_id: reply.message_id.clone(),
                in_reply_to: reply.in_reply_to.clone(),
                references: reply.references.clone(),
            }),
        };
        let sent = mailer.send(email).await;
        let failure = sent.as_ref().err().map(ToString::to_string);
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        mail::replied(&mut tx, &reply, failure.as_deref())
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        match failure {
            // Failed: retried by the step's retry policy.
            Some(e) => Err(error(
                "email",
                format!("sending to {} failed: {e}", reply.to),
            )),
            None => Ok(json!({ "sent": true, "to": reply.to, "message_id": reply.message_id })),
        }
    }

    /// `core.ticket.from_email`: a received email to ticket work, with its
    /// attachments on the ticket.
    async fn ticket_from_email(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
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

    /// `core.mail.fetch`: new mail from the IMAP mailboxes at the run's
    /// organization and below (or one), stored and announced with
    /// `email.received`. A mailbox that fails is reported, not fatal.
    async fn fetch_mail(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        self.require_desk(org_id).await?;
        let only = match &input["mailbox_id"] {
            Json::Null => None,
            v => Some(
                v.as_str()
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .ok_or_else(|| error("input", "mailbox_id is a mailbox's id"))?,
            ),
        };
        let limit = input["limit"].as_u64().map_or(50, |n| n.clamp(1, 500));
        let storage = self
            .payloads
            .storage()
            .ok_or_else(|| {
                error(
                    "unavailable",
                    "object storage isn't set up, so mail can't be kept",
                )
            })?
            .clone();
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let mailboxes = mail::fetchable(&mut tx, org_id, only)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        let mut report = Vec::with_capacity(mailboxes.len());
        for m in mailboxes {
            let result = self
                .fetch_one(&storage, &m, usize::try_from(limit).unwrap_or(50))
                .await;
            let mut tx = begin_scoped(self.db(), &OrgScope::System)
                .await
                .map_err(internal)?;
            let entry = match result {
                Ok((fetched, new, cursor)) => {
                    mail::checked(&mut tx, m.mailbox.id, Some(cursor), None)
                        .await
                        .map_err(desk_error)?;
                    json!({ "mailbox_id": m.mailbox.id, "address": m.mailbox.address, "fetched": fetched, "new": new })
                }
                Err(message) => {
                    mail::checked(&mut tx, m.mailbox.id, None, Some(&message))
                        .await
                        .map_err(desk_error)?;
                    json!({ "mailbox_id": m.mailbox.id, "address": m.mailbox.address, "error": message })
                }
            };
            tx.commit().await.map_err(internal)?;
            report.push(entry);
        }
        Ok(json!({ "mailboxes": report }))
    }

    /// Fetches one mailbox and keeps what's new: (fetched, new, cursor).
    async fn fetch_one(
        &self,
        storage: &huxley_storage::Storage,
        m: &mail::Fetchable,
        limit: usize,
    ) -> Result<(usize, usize, huxley_mail::imap::Cursor), String> {
        let password = match &m.secret {
            Some(sealed) => {
                let plain = self
                    .crypto
                    .decrypt(mail::secret_binding(m.mailbox.id), sealed)
                    .await
                    .map_err(|e| e.to_string())?;
                String::from_utf8(plain.to_vec())
                    .map_err(|_| "the stored password isn't text".to_owned())?
            }
            None => String::new(),
        };
        let config = huxley_mail::imap::Config {
            host: m.settings.host.clone(),
            port: m.settings.port.unwrap_or(993),
            security: m
                .settings
                .security
                .unwrap_or(huxley_mail::imap::Security::Tls),
            username: m.settings.username.clone(),
            password,
            folder: m.settings.folder.clone().unwrap_or_else(|| "INBOX".into()),
        };
        let (messages, cursor) =
            huxley_mail::imap::fetch(&config, m.cursor, limit, std::time::Duration::from_secs(60))
                .await
                .map_err(|e| e.to_string())?;
        let mut new = 0;
        for message in &messages {
            // Unreadable messages are skipped (they've no headers to keep).
            let Some(parsed) = huxley_mail::parse(&message.raw) else {
                continue;
            };
            let id = huxley_core::new_id();
            let key = mail::raw_key(m.mailbox.org_id, id);
            storage
                .put(&key, bytes::Bytes::from(message.raw.clone()))
                .await
                .map_err(|e| e.to_string())?;
            let mut tx = begin_scoped(self.db(), &OrgScope::System)
                .await
                .map_err(|e| e.to_string())?;
            let (_, fresh) = mail::store(&mut tx, &m.mailbox, id, &parsed, message.raw.len())
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
            if fresh {
                new += 1;
            } else {
                let _ = storage.delete(&key).await;
            }
        }
        Ok((messages.len(), new, cursor))
    }
}
