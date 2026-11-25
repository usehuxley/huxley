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
    pub(crate) async fn require_desk(&self, org_id: Uuid) -> Result<(), Json> {
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
            "core.ticket.embed" => self.embed_tickets(org_id, input).await,
            "core.ticket.from_email" => self.ticket_from_email(org_id, input).await,
            "core.ticket.email_reply" => self.email_reply(org_id, input).await,
            other => Err(internal(format!("not a service desk step: {other}"))),
        }
    }

    /// A mailer for a mailbox's own SMTP server, with its stored password.
    async fn mailbox_mailer(
        &self,
        mailbox: Uuid,
        settings: &mail::SmtpSettings,
        secret: Option<Vec<u8>>,
    ) -> Result<huxley_email::Mailer, Json> {
        let password = match secret {
            Some(sealed) => {
                let plain = self
                    .crypto
                    .decrypt(mail::secret_binding(mailbox), &sealed)
                    .await
                    .map_err(|e| {
                        error(
                            "email",
                            format!("the mailbox's password can't be read: {e}"),
                        )
                    })?;
                String::from_utf8(plain.to_vec())
                    .map_err(|_| error("email", "the mailbox's password isn't text"))?
            }
            None => String::new(),
        };
        huxley_email::Mailer::server(&huxley_email::SmtpServer {
            host: settings.host.clone(),
            port: settings.port.unwrap_or(465),
            security: match settings
                .security
                .unwrap_or(huxley_mail::imap::Security::Tls)
            {
                huxley_mail::imap::Security::Tls => huxley_email::SmtpSecurity::Tls,
                huxley_mail::imap::Security::StartTls => huxley_email::SmtpSecurity::StartTls,
                huxley_mail::imap::Security::None => huxley_email::SmtpSecurity::None,
            },
            username: settings.username.clone().unwrap_or_default(),
            password,
        })
        .map_err(|e| error("email", format!("the mailbox's SMTP server: {e}")))
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
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let planned = mail::reply_for(&mut tx, note_id, org_id)
            .await
            .map_err(desk_error)?;
        let reply = match planned {
            Ok(reply) => reply,
            Err(reason) => {
                tx.commit().await.map_err(internal)?;
                return Ok(json!({ "sent": false, "reason": reason }));
            }
        };
        let own = mail::smtp_for(&mut tx, reply.mailbox_id)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        // The mailbox's own server if it has one, else the instance's.
        let own_mailer = match own {
            Some((settings, secret)) => Some(
                self.mailbox_mailer(reply.mailbox_id, &settings, secret)
                    .await?,
            ),
            None => None,
        };
        let mailer = match &own_mailer {
            Some(m) => m,
            None => self
                .mailer
                .as_ref()
                .filter(|m| m.enabled())
                .ok_or_else(|| {
                    error(
                        "email",
                        "email isn't set up for this instance (email.smtp_url)",
                    )
                })?,
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

    /// `core.ticket.embed`: embeds stale tickets at `org_id` and below, a
    /// batch per organization by its embeddings provider. Nothing to do
    /// without pgvector. A provider that fails is reported, not fatal.
    async fn embed_tickets(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        use huxley_servicedesk::embeddings;
        self.require_desk(org_id).await?;
        let limit = input["limit"].as_i64().map_or(200, |n| n.clamp(1, 2000));
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !embeddings::available(&mut tx).await.map_err(desk_error)? {
            tx.commit().await.map_err(internal)?;
            return Ok(json!({ "available": false, "embedded": 0, "errors": [] }));
        }
        let stale = embeddings::stale(&mut tx, org_id, limit)
            .await
            .map_err(desk_error)?;
        tx.commit().await.map_err(internal)?;
        // A batch per organization (its provider), in order of first appearance.
        let mut batches: Vec<(Uuid, Vec<embeddings::Stale>)> = Vec::new();
        for ticket in stale {
            match batches.iter_mut().find(|(org, _)| *org == ticket.org_id) {
                Some((_, tickets)) => tickets.push(ticket),
                None => batches.push((ticket.org_id, vec![ticket])),
            }
        }
        let ai = self.ai();
        let purpose = huxley_ai::Purpose {
            feature: "ticket_embeddings",
            run_id: None,
        };
        let (mut embedded, mut errors) = (0_u64, Vec::new());
        for (org, tickets) in batches {
            for chunk in tickets.chunks(huxley_ai::embed::MAX_BATCH) {
                let texts: Vec<String> = chunk.iter().map(|t| t.text.clone()).collect();
                let result = ai
                    .embed(org, purpose, &texts, huxley_ai::embed::InputType::Document)
                    .await;
                let done = match result {
                    Ok(done) => done,
                    Err(huxley_ai::AiError::Internal(e)) => return Err(internal(e)),
                    Err(e) => {
                        errors.push(json!({ "org_id": org, "error": e.to_string() }));
                        break;
                    }
                };
                let mut tx = begin_scoped(self.db(), &OrgScope::System)
                    .await
                    .map_err(internal)?;
                for (ticket, vector) in chunk.iter().zip(&done.vectors) {
                    // Kept under the provider's model name, which stale() compares.
                    embeddings::save(&mut tx, ticket, &ticket.model, vector)
                        .await
                        .map_err(desk_error)?;
                    embedded += 1;
                }
                tx.commit().await.map_err(internal)?;
            }
        }
        Ok(json!({ "available": true, "embedded": embedded, "errors": errors }))
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
                    mail::checked(&mut tx, m.mailbox.id, cursor, None)
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

    /// Fetches one mailbox and keeps what's new: (fetched, new, the IMAP
    /// cursor reached, if it's IMAP).
    async fn fetch_one(
        &self,
        storage: &huxley_storage::Storage,
        m: &mail::Fetchable,
        limit: usize,
    ) -> Result<(usize, usize, Option<huxley_mail::imap::Cursor>), String> {
        match &m.source {
            mail::Source::Imap(settings) => {
                let (fetched, new, cursor) = self.fetch_imap(storage, m, settings, limit).await?;
                Ok((fetched, new, Some(cursor)))
            }
            mail::Source::Graph(settings) => {
                let (fetched, new) = self.fetch_graph(storage, m, settings, limit).await?;
                Ok((fetched, new, None))
            }
        }
    }

    async fn fetch_imap(
        &self,
        storage: &huxley_storage::Storage,
        m: &mail::Fetchable,
        settings: &mail::ImapSettings,
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
            host: settings.host.clone(),
            port: settings.port.unwrap_or(993),
            security: settings
                .security
                .unwrap_or(huxley_mail::imap::Security::Tls),
            username: settings.username.clone(),
            password,
            folder: settings.folder.clone().unwrap_or_else(|| "INBOX".into()),
        };
        let (messages, cursor) =
            huxley_mail::imap::fetch(&config, m.cursor, limit, std::time::Duration::from_secs(60))
                .await
                .map_err(|e| e.to_string())?;
        let mut new = 0;
        for message in &messages {
            if self.keep(storage, m, &message.raw).await? {
                new += 1;
            }
        }
        Ok((messages.len(), new, cursor))
    }

    /// Fetches unread mail through Microsoft Graph (the `msgraph`
    /// integration's mail actions, with the mailbox's connection): each
    /// message as MIME, kept, then marked read.
    async fn fetch_graph(
        &self,
        storage: &huxley_storage::Storage,
        m: &mail::Fetchable,
        settings: &mail::GraphSettings,
        limit: usize,
    ) -> Result<(usize, usize), String> {
        let user = settings
            .user
            .clone()
            .unwrap_or_else(|| m.mailbox.address.clone());
        let folder = settings.folder.clone().unwrap_or_else(|| "inbox".into());
        let call = async |action: &str, inputs: Json| -> Result<Json, String> {
            let request = json!({ "action": format!("msgraph/{action}"), "connection": settings.connection, "inputs": inputs });
            match self.poll_action(m.mailbox.org_id, &request).await {
                Ok(Ok(body)) => Ok(body),
                Ok(Err(failure)) => Err(format!(
                    "Microsoft Graph: {}",
                    failure["message"].as_str().unwrap_or("the call failed")
                )),
                Err(e) => Err(e.to_string()),
            }
        };
        let listed = call(
            "listMailMessages",
            json!({ "user": user, "folder": folder, "$filter": "isRead eq false",
                    "$select": "id", "$orderby": "receivedDateTime", "$top": limit }),
        )
        .await?;
        let ids: Vec<String> = listed["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect();
        let mut new = 0;
        for id in &ids {
            let mime = call("getMailMessageMime", json!({ "user": user, "message": id })).await?;
            let raw = match &mime {
                Json::String(text) => text.clone().into_bytes(),
                other => match other["base64"].as_str() {
                    Some(encoded) => {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
                            .map_err(|_| {
                                "Microsoft Graph sent a message that isn't MIME".to_owned()
                            })?
                    }
                    None => return Err("Microsoft Graph sent a message that isn't MIME".into()),
                },
            };
            if self.keep(storage, m, &raw).await? {
                new += 1;
            }
            // Read now: the next fetch doesn't see it again.
            call(
                "updateMailMessage",
                json!({ "user": user, "message": id, "body": { "isRead": true } }),
            )
            .await?;
        }
        Ok((ids.len(), new))
    }

    /// Keeps a raw message (in storage and as an email): whether it's new.
    /// Unreadable messages are skipped (they've no headers to keep).
    async fn keep(
        &self,
        storage: &huxley_storage::Storage,
        m: &mail::Fetchable,
        raw: &[u8],
    ) -> Result<bool, String> {
        let Some(parsed) = huxley_mail::parse(raw) else {
            return Ok(false);
        };
        let id = huxley_core::new_id();
        let key = mail::raw_key(m.mailbox.org_id, id);
        storage
            .put(&key, bytes::Bytes::from(raw.to_vec()))
            .await
            .map_err(|e| e.to_string())?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(|e| e.to_string())?;
        let (_, fresh) = mail::store(&mut tx, &m.mailbox, id, &parsed, raw.len())
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        if !fresh {
            let _ = storage.delete(&key).await;
        }
        Ok(fresh)
    }
}
