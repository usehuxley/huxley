//! Email intake (ADR-038): mailboxes, the messages they receive, and
//! turning a message into ticket work.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_mail::Parsed;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::{self, NewTicket, StatusChange};
use crate::{DeskError, DeskResult, text};

pub const KINDS: &[&str] = &["forward", "imap", "graph"];
pub const UNKNOWN_SENDER: &[&str] = &["create_contact", "no_requester", "ignore"];
/// More than this many messages from one sender to a mailbox in an hour
/// looks like a loop.
const LOOP_LIMIT: i64 = 20;
/// Personal-mail domains: a sender there says nothing about their company.
const PERSONAL_DOMAINS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "yahoo.com",
    "icloud.com",
    "me.com",
    "aol.com",
    "proton.me",
    "protonmail.com",
    "gmx.com",
    "gmx.de",
    "web.de",
    "mail.com",
    "yandex.com",
    "zoho.com",
];

/// Where an IMAP mailbox fetches from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImapSettings {
    pub host: String,
    /// Default 993 (TLS) or 143.
    #[serde(default)]
    pub port: Option<u16>,
    /// `tls` (default), `start_tls` or `none` (a server on your own network).
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub security: Option<huxley_mail::imap::Security>,
    pub username: String,
    /// Default `INBOX`.
    #[serde(default)]
    pub folder: Option<String>,
}

impl ImapSettings {
    /// The settings with defaults filled in, checked.
    fn checked(&self) -> DeskResult<Self> {
        let host = self.host.trim().to_lowercase();
        if host.is_empty()
            || host.len() > 253
            || host.contains(|c: char| c.is_whitespace() || c == '/')
        {
            return Err(DeskError::invalid("imap", "host is a server name"));
        }
        let security = self.security.unwrap_or(huxley_mail::imap::Security::Tls);
        let port = self
            .port
            .unwrap_or(if security == huxley_mail::imap::Security::Tls {
                993
            } else {
                143
            });
        if port == 0 {
            return Err(DeskError::invalid("imap", "port is 1 to 65535"));
        }
        let username = self.username.trim().to_owned();
        if username.is_empty() || username.len() > 320 {
            return Err(DeskError::invalid("imap", "username is needed"));
        }
        let folder = self
            .folder
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .unwrap_or("INBOX");
        Ok(Self {
            host,
            port: Some(port),
            security: Some(security),
            username,
            folder: Some(folder.to_owned()),
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub address: String,
    /// `forward` (posted to Huxley), `imap` or `graph` (fetched).
    pub kind: String,
    /// The type of new tickets (a ticket type's key).
    pub ticket_type: String,
    pub queue_id: Option<Uuid>,
    /// `create_contact`, `no_requester` or `ignore`.
    pub unknown_sender: String,
    /// Where unknown senders' tickets go; none: the mailbox's organization.
    pub unknown_org_id: Option<Uuid>,
    pub strip_replies: bool,
    pub enabled: bool,
    /// Where it fetches from (`imap` mailboxes).
    #[sqlx(json(nullable))]
    pub imap: Option<ImapSettings>,
    /// A password is stored (it's never shown).
    pub has_password: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_checked_at: Option<OffsetDateTime>,
    pub last_error: Option<String>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailboxInput {
    pub name: String,
    pub address: String,
    /// `forward` (default), `imap` or `graph`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub ticket_type: Option<String>,
    #[serde(default)]
    pub queue_id: Option<Uuid>,
    #[serde(default)]
    pub unknown_sender: Option<String>,
    #[serde(default)]
    pub unknown_org_id: Option<Uuid>,
    #[serde(default)]
    pub strip_replies: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Where to fetch from (`imap` mailboxes).
    #[serde(default)]
    pub imap: Option<ImapSettings>,
    /// The IMAP password; left out, the stored one stays.
    #[serde(default)]
    #[schema(format = Password, write_only)]
    pub password: Option<String>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT id, org_id, name, address, kind, ticket_type, queue_id, unknown_sender, \
    unknown_org_id, strip_replies, enabled, imap, secret IS NOT NULL AS has_password, last_checked_at, \
    last_error, version, created_at \
    FROM sd_mailboxes";

pub async fn mailboxes(conn: &mut PgConnection) -> DeskResult<Vec<Mailbox>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE archived_at IS NULL ORDER BY lower(name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn mailbox(conn: &mut PgConnection, id: Uuid) -> DeskResult<Mailbox> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE id = $1 AND archived_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(DeskError::NotFound)
}

struct Checked {
    name: String,
    address: String,
    kind: String,
    ticket_type: String,
    unknown_sender: String,
    imap: Option<ImapSettings>,
}

async fn check(conn: &mut PgConnection, input: &MailboxInput) -> DeskResult<Checked> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| DeskError::invalid("name", "a mailbox needs a name"))?;
    let address = input.address.trim().to_lowercase();
    let valid = address.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !address.contains(char::is_whitespace)
    });
    if !valid || address.len() > 254 {
        return Err(DeskError::invalid("address", "an email address"));
    }
    let kind = input.kind.clone().unwrap_or_else(|| "forward".into());
    if !KINDS.contains(&kind.as_str()) {
        return Err(DeskError::invalid("kind", KINDS.join(", ")));
    }
    let unknown_sender = input
        .unknown_sender
        .clone()
        .unwrap_or_else(|| "create_contact".into());
    if !UNKNOWN_SENDER.contains(&unknown_sender.as_str()) {
        return Err(DeskError::invalid(
            "unknownSender",
            UNKNOWN_SENDER.join(", "),
        ));
    }
    let ticket_type = input
        .ticket_type
        .clone()
        .unwrap_or_else(|| "incident".into());
    crate::config::type_by_key(conn, &ticket_type)
        .await
        .map_err(|_| {
            DeskError::invalid(
                "ticketType",
                format!("there's no ticket type {ticket_type:?}"),
            )
        })?;
    if let Some(queue) = input.queue_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sd_queues WHERE id = $1 AND archived_at IS NULL)",
        )
        .bind(queue)
        .fetch_one(&mut *conn)
        .await?;
        if !ok {
            return Err(DeskError::invalid("queueId", "there's no such queue"));
        }
    }
    let imap = match (&input.imap, kind.as_str()) {
        (Some(settings), "imap") => Some(settings.checked()?),
        (None, "imap") => {
            return Err(DeskError::invalid(
                "imap",
                "an IMAP mailbox needs its server",
            ));
        }
        _ => None,
    };
    Ok(Checked {
        name,
        address,
        kind,
        ticket_type,
        unknown_sender,
        imap,
    })
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_mailboxes_address") => {
            DeskError::invalid("address", "another mailbox has this address")
        }
        sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
            DeskError::invalid("unknownOrgId", "there's no such organization")
        }
        _ => DeskError::Sqlx(e),
    }
}

/// Creates a mailbox (system scope; the caller checks the organization).
/// The binding a mailbox's password is sealed with.
pub const fn secret_binding(mailbox: Uuid) -> huxley_crypto::Binding<'static> {
    huxley_crypto::Binding {
        purpose: "sd_mailbox.password",
        record: mailbox,
    }
}

/// Creates mailbox `id` (system scope; the caller checks the organization).
/// `secret` is its password, sealed with [`secret_binding`].
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    id: Uuid,
    input: &MailboxInput,
    secret: Option<&[u8]>,
) -> DeskResult<Mailbox> {
    let c = check(conn, input).await?;
    sqlx::query(
        "INSERT INTO sd_mailboxes (id, org_id, name, address, kind, ticket_type, queue_id, unknown_sender, \
                                   unknown_org_id, strip_replies, enabled, imap, secret) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.name)
    .bind(&c.address)
    .bind(&c.kind)
    .bind(&c.ticket_type)
    .bind(input.queue_id)
    .bind(&c.unknown_sender)
    .bind(input.unknown_org_id)
    .bind(input.strip_replies.unwrap_or(true))
    .bind(input.enabled.unwrap_or(true))
    .bind(c.imap.as_ref().map(|i| json!(i)))
    .bind(secret)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("mailbox.created")
            .org(org_id)
            .target("mailbox", id)
            .details(json!({ "name": c.name, "address": c.address, "kind": c.kind })),
    )
    .await?;
    mailbox(conn, id).await
}

/// Changes a mailbox. `secret`: a new sealed password (`None` keeps it).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &MailboxInput,
    secret: Option<&[u8]>,
) -> DeskResult<Mailbox> {
    let c = check(conn, input).await?;
    let before = mailbox(conn, id).await?;
    let rows = sqlx::query(
        "UPDATE sd_mailboxes SET name = $3, address = $4, kind = $5, ticket_type = $6, queue_id = $7, \
                unknown_sender = $8, unknown_org_id = $9, strip_replies = $10, enabled = $11, \
                imap = $12, secret = coalesce($13, secret), \
                cursor = CASE WHEN imap IS DISTINCT FROM $12 THEN '{}'::jsonb ELSE cursor END, \
                version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&c.name)
    .bind(&c.address)
    .bind(&c.kind)
    .bind(&c.ticket_type)
    .bind(input.queue_id)
    .bind(&c.unknown_sender)
    .bind(input.unknown_org_id)
    .bind(input.strip_replies.unwrap_or(before.strip_replies))
    .bind(input.enabled.unwrap_or(before.enabled))
    .bind(c.imap.as_ref().map(|i| json!(i)))
    .bind(secret)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("mailbox.updated")
            .org(before.org_id)
            .target("mailbox", id)
            .details(json!({ "name": c.name, "address": c.address })),
    )
    .await?;
    mailbox(conn, id).await
}

pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let m = mailbox(conn, id).await?;
    sqlx::query("UPDATE sd_mailboxes SET archived_at = now(), enabled = false, version = version + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("mailbox.archived")
            .org(m.org_id)
            .target("mailbox", id)
            .details(json!({ "name": m.name, "address": m.address })),
    )
    .await?;
    Ok(())
}

/// An IMAP mailbox to fetch from: its settings, sealed password and cursor.
#[derive(Debug, Clone)]
pub struct Fetchable {
    pub mailbox: Mailbox,
    pub settings: ImapSettings,
    pub secret: Option<Vec<u8>>,
    pub cursor: huxley_mail::imap::Cursor,
}

/// Enabled IMAP mailboxes at `within` and below (or just `only`), oldest
/// check first. System scope.
pub async fn fetchable(
    conn: &mut PgConnection,
    within: Uuid,
    only: Option<Uuid>,
) -> DeskResult<Vec<Fetchable>> {
    let ids: Vec<(Uuid, Option<Vec<u8>>, Json)> = sqlx::query_as(
        "SELECT m.id, m.secret, m.cursor FROM sd_mailboxes m \
           JOIN organizations o ON o.id = m.org_id JOIN organizations w ON w.id = $1 \
          WHERE m.kind = 'imap' AND m.enabled AND m.archived_at IS NULL AND o.path <@ w.path \
            AND ($2::uuid IS NULL OR m.id = $2) \
          ORDER BY m.last_checked_at NULLS FIRST, m.id",
    )
    .bind(within)
    .bind(only)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for (id, secret, cursor) in ids {
        let mailbox = mailbox(conn, id).await?;
        let Some(settings) = mailbox.imap.clone() else {
            continue;
        };
        out.push(Fetchable {
            mailbox,
            settings,
            secret,
            cursor: serde_json::from_value(cursor).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Records a check: the cursor reached (none: unchanged) and the error, if
/// it failed.
pub async fn checked(
    conn: &mut PgConnection,
    id: Uuid,
    cursor: Option<huxley_mail::imap::Cursor>,
    error: Option<&str>,
) -> DeskResult<()> {
    sqlx::query(
        "UPDATE sd_mailboxes SET last_checked_at = now(), last_error = $3, \
                cursor = coalesce($2, cursor) WHERE id = $1",
    )
    .bind(id)
    .bind(cursor.map(|c| json!(c)))
    .bind(error)
    .execute(conn)
    .await?;
    Ok(())
}

/// A received message, as kept.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Email {
    pub id: Uuid,
    pub org_id: Uuid,
    pub mailbox_id: Uuid,
    pub message_id: String,
    pub from_address: Option<String>,
    pub from_name: Option<String>,
    pub subject: String,
    pub body_text: String,
    /// Why it looks automatic, if it does.
    pub automatic: Option<String>,
    pub size_bytes: i64,
    /// `[{filename, contentType, size, inline}]`
    #[schema(value_type = Vec<Object>)]
    pub attachments: Json,
    #[serde(with = "time::serde::rfc3339")]
    pub received_at: OffsetDateTime,
    /// What became of it: `ticket_created`, `note_added`, `ticket_reopened`,
    /// `ignored_automatic`, `ignored_loop` or `ignored_unknown_sender`.
    pub outcome: Option<String>,
    pub outcome_detail: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub processed_at: Option<OffsetDateTime>,
    pub ticket_id: Option<Uuid>,
    /// Where the raw message is stored.
    #[serde(skip)]
    pub raw_key: String,
}

const EMAIL: &str = "SELECT id, org_id, mailbox_id, message_id, from_address, from_name, subject, body_text, \
    automatic, size_bytes, attachments, received_at, outcome, outcome_detail, processed_at, ticket_id, raw_key \
    FROM sd_inbound_emails";

pub async fn email(conn: &mut PgConnection, id: Uuid) -> DeskResult<Email> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{EMAIL} WHERE id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

/// A mailbox's latest emails, newest first.
pub async fn emails(conn: &mut PgConnection, mailbox: Uuid, limit: i64) -> DeskResult<Vec<Email>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{EMAIL} WHERE mailbox_id = $1 ORDER BY received_at DESC, id DESC LIMIT $2"
    )))
    .bind(mailbox)
    .bind(limit.clamp(1, 200))
    .fetch_all(conn)
    .await?)
}

/// Where a mailbox keeps a message's raw bytes.
pub fn raw_key(org_id: Uuid, email_id: Uuid) -> String {
    format!("mail/{org_id}/{email_id}.eml")
}

/// Keeps a received message (already stored at [`raw_key`] under `id`) and
/// emits `email.received`. Returns the email's id and whether it's new: a
/// message the mailbox already has (by `Message-ID`) is kept once.
pub async fn store(
    conn: &mut PgConnection,
    mailbox: &Mailbox,
    id: Uuid,
    parsed: &Parsed,
    size: usize,
) -> DeskResult<(Uuid, bool)> {
    let attachments: Vec<Json> = parsed
        .attachments
        .iter()
        .map(|a| json!({ "filename": a.filename, "contentType": a.content_type, "size": a.data.len(), "inline": a.inline }))
        .collect();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO sd_inbound_emails (id, org_id, mailbox_id, message_id, in_reply_to, refs, from_address, \
                                        from_name, to_addresses, cc_addresses, subject, body_text, automatic, \
                                        raw_key, size_bytes, attachments) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
         ON CONFLICT (mailbox_id, message_id) DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(mailbox.org_id)
    .bind(mailbox.id)
    .bind(&parsed.message_id)
    .bind(&parsed.in_reply_to)
    .bind(&parsed.references)
    .bind(parsed.from.as_ref().map(|f| &f.address))
    .bind(parsed.from.as_ref().and_then(|f| f.name.as_ref()))
    .bind(json!(parsed.to))
    .bind(json!(parsed.cc))
    .bind(&parsed.subject)
    .bind(&parsed.text)
    .bind(parsed.automatic)
    .bind(raw_key(mailbox.org_id, id))
    .bind(i64::try_from(size).unwrap_or(i64::MAX))
    .bind(Json::Array(attachments))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = inserted else {
        let existing: Uuid = sqlx::query_scalar(
            "SELECT id FROM sd_inbound_emails WHERE mailbox_id = $1 AND message_id = $2",
        )
        .bind(mailbox.id)
        .bind(&parsed.message_id)
        .fetch_one(&mut *conn)
        .await?;
        return Ok((existing, false));
    };
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "email.received",
            org_id: Some(mailbox.org_id),
            subject_type: "email",
            subject_id: id,
            payload: json!({
                "email_id": id,
                "mailbox_id": mailbox.id,
                "from": parsed.from.as_ref().map(|f| &f.address),
                "subject": parsed.subject,
            }),
        },
    )
    .await?;
    Ok((id, true))
}

/// What a message became.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Outcome {
    pub outcome: String,
    pub detail: Option<String>,
    pub ticket_id: Option<Uuid>,
    /// A new ticket was opened.
    pub created: bool,
    /// Processed just now (not an earlier result): its attachments still
    /// need to go on the ticket.
    #[serde(skip)]
    pub fresh: bool,
}

impl Outcome {
    fn ignored(outcome: &str, detail: impl Into<String>) -> Self {
        Self {
            outcome: outcome.into(),
            detail: Some(detail.into()),
            ticket_id: None,
            created: false,
            fresh: true,
        }
    }
}

async fn finish(conn: &mut PgConnection, email: Uuid, outcome: Outcome) -> DeskResult<Outcome> {
    sqlx::query(
        "UPDATE sd_inbound_emails SET outcome = $2, outcome_detail = $3, ticket_id = $4, processed_at = now() \
          WHERE id = $1",
    )
    .bind(email)
    .bind(&outcome.outcome)
    .bind(&outcome.detail)
    .bind(outcome.ticket_id)
    .execute(conn)
    .await?;
    Ok(outcome)
}

/// Whether `org_id` is `within` or below it.
async fn under(conn: &mut PgConnection, org_id: Uuid, within: Uuid) -> DeskResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT o.path <@ w.path FROM organizations o, organizations w WHERE o.id = $1 AND w.id = $2",
    )
    .bind(org_id)
    .bind(within)
    .fetch_optional(conn)
    .await?
    .unwrap_or(false))
}

/// The ticket a message replies to: by subject token, else by the ids it
/// refers to. Only tickets at `within` or below.
async fn thread(
    conn: &mut PgConnection,
    email: &Email,
    within: Uuid,
) -> DeskResult<Option<tickets::Ticket>> {
    let mut candidate: Option<Uuid> = None;
    if let Some(number) = huxley_mail::ticket_token(&email.subject) {
        candidate = sqlx::query_scalar("SELECT id FROM sd_tickets WHERE number = $1")
            .bind(number)
            .fetch_optional(&mut *conn)
            .await?;
    }
    if candidate.is_none() {
        // A reply to one of our emailed notes.
        candidate = sqlx::query_scalar(
            "SELECT n.ticket_id FROM sd_ticket_notes n, sd_inbound_emails me \
              WHERE me.id = $1 AND n.message_id = ANY (me.in_reply_to || me.refs) \
              ORDER BY n.created_at DESC LIMIT 1",
        )
        .bind(email.id)
        .fetch_optional(&mut *conn)
        .await?;
    }
    if candidate.is_none() {
        candidate = sqlx::query_scalar(
            "SELECT e.ticket_id FROM sd_inbound_emails e, sd_inbound_emails me \
              WHERE me.id = $1 AND e.id <> me.id AND e.ticket_id IS NOT NULL \
                AND e.message_id = ANY (me.in_reply_to || me.refs) \
              ORDER BY e.received_at DESC LIMIT 1",
        )
        .bind(email.id)
        .fetch_optional(&mut *conn)
        .await?;
    }
    let Some(id) = candidate else { return Ok(None) };
    let ticket = tickets::get(conn, id).await?;
    Ok(under(conn, ticket.org_id, within).await?.then_some(ticket))
}

/// The company for an unknown sender: where most contacts share their
/// email domain (not a personal-mail one), at `within` or below.
async fn company_by_domain(
    conn: &mut PgConnection,
    address: &str,
    within: Uuid,
) -> DeskResult<Option<Uuid>> {
    let Some((_, domain)) = address.rsplit_once('@') else {
        return Ok(None);
    };
    if PERSONAL_DOMAINS.contains(&domain) {
        return Ok(None);
    }
    Ok(sqlx::query_scalar(
        "SELECT c.org_id FROM crm_contacts c \
           JOIN organizations o ON o.id = c.org_id JOIN organizations w ON w.id = $2 \
          WHERE lower(split_part(c.email, '@', 2)) = $1 AND c.archived_at IS NULL AND o.path <@ w.path \
          GROUP BY c.org_id ORDER BY count(*) DESC, c.org_id LIMIT 1",
    )
    .bind(domain)
    .bind(within)
    .fetch_optional(conn)
    .await?)
}

/// Splits a display name into first and last.
fn names(name: Option<&str>, address: &str) -> (String, String) {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => match n.rsplit_once(' ') {
            Some((first, last)) => (first.trim().to_owned(), last.trim().to_owned()),
            None => (n.to_owned(), String::new()),
        },
        None => (
            address.split('@').next().unwrap_or(address).to_owned(),
            String::new(),
        ),
    }
}

/// Who sent a message: their contact (made per the mailbox's policy) and
/// the organization a new ticket goes to. `None`: ignore the message.
async fn sender(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    mailbox: &Mailbox,
    email: &Email,
    within: Uuid,
) -> DeskResult<Option<(Option<Uuid>, Uuid)>> {
    let address = email.from_address.clone().unwrap_or_default();
    for contact in huxley_crm::contacts::find_by_email(conn, &address)
        .await
        .map_err(crm)?
    {
        if under(conn, contact.org_id, within).await? {
            return Ok(Some((Some(contact.id), contact.org_id)));
        }
    }
    if mailbox.unknown_sender == "ignore" || address.is_empty() {
        return Ok(None);
    }
    let fallback = match mailbox.unknown_org_id {
        Some(org) if under(conn, org, within).await? => org,
        _ => mailbox.org_id,
    };
    let org = company_by_domain(conn, &address, within)
        .await?
        .unwrap_or(fallback);
    if mailbox.unknown_sender == "no_requester" {
        return Ok(Some((None, org)));
    }
    let (first_name, last_name) = names(email.from_name.as_deref(), &address);
    let input = huxley_crm::contacts::ContactInput {
        first_name,
        last_name,
        email: Some(address),
        ..Default::default()
    };
    let contact = huxley_crm::contacts::create(conn, ctx, org, &input)
        .await
        .map_err(crm)?;
    Ok(Some((Some(contact.id), org)))
}

fn crm(e: huxley_crm::CrmError) -> DeskError {
    match e {
        huxley_crm::CrmError::NotFound => DeskError::NotFound,
        huxley_crm::CrmError::Invalid { field, message } => DeskError::Invalid { field, message },
        huxley_crm::CrmError::VersionConflict => DeskError::VersionConflict,
        huxley_crm::CrmError::Db(e) => DeskError::Db(e),
        huxley_crm::CrmError::Sqlx(e) => DeskError::Sqlx(e),
    }
}

/// Reopens a resolved ticket (to its type's first open status).
async fn reopen(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &tickets::Ticket,
) -> DeskResult<()> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT s.key FROM sd_statuses s JOIN sd_tickets t ON t.type_id = s.type_id \
          WHERE t.id = $1 AND s.category = 'open' ORDER BY s.position LIMIT 1",
    )
    .bind(ticket.id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(status) = status {
        let change = StatusChange {
            status,
            resolution: None,
            version: ticket.version,
        };
        tickets::change_status(conn, ctx, ticket.id, &change).await?;
    }
    Ok(())
}

/// Why a message shouldn't become ticket work: automatic mail, or a loop.
async fn screen(
    conn: &mut PgConnection,
    mailbox: &Mailbox,
    email: &Email,
) -> DeskResult<Option<Outcome>> {
    if let Some(why) = &email.automatic {
        return Ok(Some(Outcome::ignored("ignored_automatic", why.clone())));
    }
    let from = email.from_address.clone().unwrap_or_default();
    let (ours, recent): (bool, i64) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM sd_mailboxes WHERE address = $2 AND archived_at IS NULL), \
                (SELECT count(*) FROM sd_inbound_emails WHERE mailbox_id = $1 AND from_address = $2 \
                    AND received_at > now() - interval '1 hour')",
    )
    .bind(mailbox.id)
    .bind(&from)
    .fetch_one(conn)
    .await?;
    if ours {
        return Ok(Some(Outcome::ignored(
            "ignored_loop",
            "sent by one of Huxley's mailboxes",
        )));
    }
    if recent > LOOP_LIMIT {
        let detail = format!("more than {LOOP_LIMIT} messages from {from} in an hour");
        return Ok(Some(Outcome::ignored("ignored_loop", detail)));
    }
    Ok(None)
}

/// Adds a reply to its ticket, reopening it if it was resolved.
async fn reply(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: &tickets::Ticket,
    email: &Email,
    body: &str,
) -> DeskResult<Outcome> {
    let from = email.from_address.clone().unwrap_or_default();
    let contact = huxley_crm::contacts::find_by_email(conn, &from)
        .await
        .map_err(crm)?
        .into_iter()
        .map(|c| c.id)
        .next();
    let reopened = ticket.status_category == "resolved";
    if reopened {
        reopen(conn, ctx, ticket).await?;
    }
    let ticket = tickets::get(conn, ticket.id).await?;
    tickets::add_email_note(conn, ctx, &ticket, &from, contact, body, email.id).await?;
    Ok(Outcome {
        outcome: if reopened {
            "ticket_reopened"
        } else {
            "note_added"
        }
        .into(),
        detail: Some(format!("added to {}", ticket.reference)),
        ticket_id: Some(ticket.id),
        created: false,
        fresh: true,
    })
}

/// Opens a ticket for a message, for its sender (per the mailbox's policy).
async fn open(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    mailbox: &Mailbox,
    email: &Email,
    body: String,
    within: Uuid,
) -> DeskResult<Outcome> {
    let Some((requester, org_id)) = sender(conn, ctx, mailbox, email, within).await? else {
        let from = email.from_address.clone().unwrap_or_default();
        return Ok(Outcome::ignored(
            "ignored_unknown_sender",
            format!("{from} isn't a contact"),
        ));
    };
    let title = huxley_mail::clean_subject(&email.subject);
    let input = NewTicket {
        org_id,
        r#type: Some(mailbox.ticket_type.clone()),
        title: if title.is_empty() {
            "(no subject)".into()
        } else {
            title.chars().take(300).collect()
        },
        description: Some(body),
        queue_id: mailbox.queue_id,
        requester_id: requester,
        source: Some("email".into()),
        ..Default::default()
    };
    let prefix = tickets::prefix_for(conn, org_id).await?;
    let plan = crate::sla::plan_for(conn, org_id).await?;
    let ticket = tickets::create(conn, ctx, &input, &prefix, plan.as_ref()).await?;
    Ok(Outcome {
        outcome: "ticket_created".into(),
        detail: Some(format!("opened {}", ticket.reference)),
        ticket_id: Some(ticket.id),
        created: true,
        fresh: true,
    })
}

/// Turns a stored email into ticket work: skips automatic mail and loops,
/// adds a reply to the ticket it belongs to (still open, or resolved, which
/// it reopens), or opens a new ticket. Works only with organizations at
/// `within` (the run's) or below. Runs in system scope; a processed email
/// returns what it became before.
pub async fn to_ticket(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email_id: Uuid,
    within: Uuid,
) -> DeskResult<Outcome> {
    let email = email(conn, email_id).await?;
    if !under(conn, email.org_id, within).await? {
        return Err(DeskError::NotFound);
    }
    if let Some(outcome) = &email.outcome {
        return Ok(Outcome {
            outcome: outcome.clone(),
            detail: email.outcome_detail.clone(),
            ticket_id: email.ticket_id,
            created: false,
            fresh: false,
        });
    }
    let mailbox =
        sqlx::query_as::<_, Mailbox>(sqlx::AssertSqlSafe(format!("{SELECT} WHERE id = $1")))
            .bind(email.mailbox_id)
            .fetch_one(&mut *conn)
            .await?;
    let outcome = if let Some(skip) = screen(conn, &mailbox, &email).await? {
        skip
    } else {
        let body = if mailbox.strip_replies {
            huxley_mail::strip_reply(&email.body_text)
        } else {
            email.body_text.trim().to_owned()
        };
        match thread(conn, &email, within).await? {
            Some(ticket) if ticket.status_category != "closed" => {
                reply(conn, ctx, &ticket, &email, &body).await?
            }
            _ => open(conn, ctx, &mailbox, &email, body, within).await?,
        }
    };
    finish(conn, email.id, outcome).await
}

#[derive(sqlx::FromRow)]
struct NoteRow {
    ticket_id: Uuid,
    kind: String,
    body: String,
    email_id: Option<Uuid>,
    author_user_id: Option<Uuid>,
    emailed_at: Option<OffsetDateTime>,
    author_name: Option<String>,
}

/// A public note to email to the requester.
#[derive(Debug, Clone)]
pub struct Reply {
    pub note_id: Uuid,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub from_name: String,
    pub from_address: String,
    pub message_id: String,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
}

/// What to email for note `note_id`, or why nothing (`Err`). A note is
/// emailed once: a public note written in Huxley, on a ticket whose
/// requester has an address, from the mailbox the ticket's mail came to
/// (else the nearest one at its organization or above). System scope;
/// tickets at `within` and below.
pub async fn reply_for(
    conn: &mut PgConnection,
    note_id: Uuid,
    within: Uuid,
) -> DeskResult<Result<Reply, &'static str>> {
    let note: Option<NoteRow> = sqlx::query_as(
        "SELECT n.ticket_id, n.kind, n.body, n.email_id, n.author_user_id, n.emailed_at, \
                u.display_name AS author_name \
           FROM sd_ticket_notes n LEFT JOIN users u ON u.id = n.author_user_id WHERE n.id = $1",
    )
    .bind(note_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(NoteRow {
        ticket_id,
        kind,
        body,
        email_id,
        author_user_id: author,
        emailed_at,
        author_name,
    }) = note
    else {
        return Err(DeskError::NotFound);
    };
    let ticket = tickets::get(conn, ticket_id).await?;
    if !under(conn, ticket.org_id, within).await? {
        return Err(DeskError::NotFound);
    }
    if kind != "public" {
        return Ok(Err("only public notes are emailed"));
    }
    if email_id.is_some() || author.is_none() {
        return Ok(Err("the note came from the requester"));
    }
    if emailed_at.is_some() {
        return Ok(Err("already emailed"));
    }
    let Some(to) = ticket.requester_email.clone() else {
        return Ok(Err("the ticket has no requester with an email address"));
    };
    // The mailbox the ticket's mail came to, else the nearest one above.
    let mailbox: Option<(String, String)> = sqlx::query_as(
        "SELECT m.name, m.address FROM sd_inbound_emails e JOIN sd_mailboxes m ON m.id = e.mailbox_id \
          WHERE e.ticket_id = $1 AND m.archived_at IS NULL AND m.enabled \
          ORDER BY e.received_at DESC LIMIT 1",
    )
    .bind(ticket.id)
    .fetch_optional(&mut *conn)
    .await?;
    let mailbox = match mailbox {
        Some(m) => Some(m),
        None => {
            sqlx::query_as(
                "SELECT m.name, m.address FROM sd_mailboxes m \
                   JOIN organizations o ON o.id = m.org_id JOIN organizations t ON t.id = $1 \
                  WHERE t.path <@ o.path AND m.archived_at IS NULL AND m.enabled \
                  ORDER BY nlevel(o.path) DESC, m.created_at LIMIT 1",
            )
            .bind(ticket.org_id)
            .fetch_optional(&mut *conn)
            .await?
        }
    };
    let Some((mailbox_name, from_address)) = mailbox else {
        return Ok(Err("there's no mailbox to send from"));
    };
    // The conversation so far: what came in and what we sent.
    let thread: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM ( \
           SELECT message_id AS id, received_at AS at FROM sd_inbound_emails WHERE ticket_id = $1 \
           UNION ALL \
           SELECT message_id, created_at FROM sd_ticket_notes \
            WHERE ticket_id = $1 AND message_id IS NOT NULL) t \
          ORDER BY at DESC LIMIT 20",
    )
    .bind(ticket.id)
    .fetch_all(&mut *conn)
    .await?;
    let domain = from_address
        .rsplit_once('@')
        .map_or_else(|| "huxley.invalid".to_owned(), |(_, d)| d.to_owned());
    let signature =
        author_name.map_or_else(|| mailbox_name.clone(), |a| format!("{a}\n{mailbox_name}"));
    Ok(Ok(Reply {
        note_id,
        to,
        subject: huxley_mail::with_token(&format!("Re: {}", ticket.title), ticket.number),
        text: format!(
            "{body}\n\n{signature}\n\n--\nReply to this email to add to your request {}.",
            ticket.reference
        ),
        from_name: mailbox_name,
        from_address,
        message_id: format!("hx.{note_id}@{domain}"),
        in_reply_to: thread.first().cloned().into_iter().collect(),
        references: thread.into_iter().rev().collect(),
    }))
}

/// Records that a note was emailed (or why it couldn't be).
pub async fn replied(
    conn: &mut PgConnection,
    reply: &Reply,
    error: Option<&str>,
) -> DeskResult<()> {
    sqlx::query(
        "UPDATE sd_ticket_notes SET message_id = CASE WHEN $4::text IS NULL THEN $2 END, \
                emailed_to = $3, emailed_at = CASE WHEN $4::text IS NULL THEN now() END, email_error = $4 \
          WHERE id = $1",
    )
    .bind(reply.note_id)
    .bind(&reply.message_id)
    .bind(&reply.to)
    .bind(error)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_split() {
        assert_eq!(
            names(Some("Ada King Lovelace"), "a@x.io"),
            ("Ada King".into(), "Lovelace".into())
        );
        assert_eq!(names(Some("Ada"), "a@x.io"), ("Ada".into(), String::new()));
        assert_eq!(names(None, "ada.l@x.io"), ("ada.l".into(), String::new()));
        assert_eq!(names(Some("  "), "bob@x.io"), ("bob".into(), String::new()));
    }
}
