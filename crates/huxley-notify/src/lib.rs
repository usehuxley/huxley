//! Notifications (design doc §8.3): a central service with an in-app channel
//! (delivered in real time) and an email channel, and per-user preferences by
//! notification kind.
//!
//! - [`notify`] records a notification inside the caller's transaction, so it
//!   exists only if the change that caused it committed. It also signals
//!   `pg_notify('huxley_notifications', <user id>)`, which Postgres delivers on
//!   commit to every API process's [`Broadcaster`] (fan-out without Valkey).
//! - Email is queued on the notification row and sent by
//!   [`run_email_dispatcher`] in the worker role, with retries. Notification
//!   emails never contain secrets.
//! - Security notices are mandatory: users can't turn them off, because an
//!   email saying "your password changed" is how they'd learn it wasn't them.
//!
//! Later phases add Teams/Slack and web push channels, and notification rules
//! as system workflows.

use std::time::Duration as StdDuration;

use huxley_email::{Mailer, action_email};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgListener;
use sqlx::{PgConnection, PgPool};
use time::{Duration, OffsetDateTime};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use utoipa::ToSchema;
use uuid::Uuid;

/// The Postgres channel for real-time delivery. Payload: the user id.
pub const CHANNEL: &str = "huxley_notifications";

/// A kind of notification and its default channels.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Kind {
    pub key: &'static str,
    pub description: &'static str,
    pub in_app: bool,
    pub email: bool,
    /// Can't be turned off (security notices).
    pub mandatory: bool,
    /// The module it belongs to: offered only while that module is on.
    #[serde(skip)]
    pub module: Option<&'static str>,
}

pub mod kinds {
    use super::Kind;

    pub const PASSWORD_CHANGED: &str = "security.password_changed";
    pub const MFA_CHANGED: &str = "security.mfa_changed";
    pub const API_TOKEN_CREATED: &str = "security.api_token_created";
    pub const OAUTH_APP_CONNECTED: &str = "security.oauth_app_connected";
    pub const ROLE_GRANTED: &str = "access.role_granted";
    pub const TEAM_JOINED: &str = "access.team_joined";
    pub const WORKFLOW: &str = "workflow.notification";
    pub const TICKET_ASSIGNED: &str = "ticket.assigned";
    pub const TICKET_REPLIED: &str = "ticket.replied";
    pub const TICKET_SLA: &str = "ticket.sla";
    pub const QUOTE_MENTIONED: &str = "quote.mentioned";
    pub const QUOTE_REVIEWED: &str = "quote.reviewed";
    pub const INVOICE_MENTIONED: &str = "invoice.mentioned";
    pub const INVOICE_REVIEWED: &str = "invoice.reviewed";
    pub const EXPENSE_MENTIONED: &str = "expense.mentioned";
    pub const EXPENSE_REVIEWED: &str = "expense.reviewed";
    pub const SALES_TASK: &str = "sales.task";

    pub const ALL: &[Kind] = &[
        Kind {
            key: PASSWORD_CHANGED,
            description: "Your password was changed or reset",
            in_app: true,
            email: true,
            mandatory: true,
            module: None,
        },
        Kind {
            key: MFA_CHANGED,
            description: "Your two-factor authentication or passkeys changed",
            in_app: true,
            email: true,
            mandatory: true,
            module: None,
        },
        Kind {
            key: API_TOKEN_CREATED,
            description: "An API token was created for your account",
            in_app: true,
            email: true,
            mandatory: true,
            module: None,
        },
        Kind {
            key: OAUTH_APP_CONNECTED,
            description: "An app (such as an AI client) was connected to your account",
            in_app: true,
            email: true,
            mandatory: true,
            module: None,
        },
        Kind {
            key: ROLE_GRANTED,
            description: "You were given a role (directly or through a team)",
            in_app: true,
            email: false,
            mandatory: false,
            module: None,
        },
        Kind {
            key: TEAM_JOINED,
            description: "You were added to a team",
            in_app: true,
            email: false,
            mandatory: false,
            module: None,
        },
        Kind {
            key: WORKFLOW,
            description: "Notifications sent by workflows",
            in_app: true,
            email: true,
            mandatory: false,
            module: None,
        },
        Kind {
            key: TICKET_ASSIGNED,
            description: "A ticket was assigned to you or your team",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("servicedesk"),
        },
        Kind {
            key: TICKET_REPLIED,
            description: "A requester replied to a ticket of yours",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("servicedesk"),
        },
        Kind {
            key: TICKET_SLA,
            description: "A ticket of yours is close to, or past, an SLA target",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("servicedesk"),
        },
        Kind {
            key: QUOTE_MENTIONED,
            description: "You were mentioned in a comment on a quote",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("sales"),
        },
        Kind {
            key: QUOTE_REVIEWED,
            description: "A quote of yours was approved, sent back for changes or rejected in review",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("sales"),
        },
        Kind {
            key: INVOICE_MENTIONED,
            description: "You were mentioned in a comment on an invoice",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("billing"),
        },
        Kind {
            key: INVOICE_REVIEWED,
            description: "An invoice you submitted was approved, sent back for changes or rejected in review",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("billing"),
        },
        Kind {
            key: EXPENSE_MENTIONED,
            description: "Someone commented on an expense report of yours, or mentioned you on one",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("expenses"),
        },
        Kind {
            key: EXPENSE_REVIEWED,
            description: "An expense report of yours was approved, sent back or rejected",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("expenses"),
        },
        Kind {
            key: SALES_TASK,
            description: "Someone gave you a sales task",
            in_app: true,
            email: true,
            mandatory: false,
            module: Some("sales"),
        },
    ];

    /// The kinds a workflow's `core.notify` may send as: not security or
    /// access notices, which only Huxley sends.
    pub fn sendable(key: &str) -> Option<&'static Kind> {
        get(key).filter(|k| k.key == WORKFLOW || k.module.is_some())
    }

    pub fn get(key: &str) -> Option<&'static Kind> {
        ALL.iter().find(|k| k.key == key)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("unknown notification kind `{0}`")]
    UnknownKind(String),
    #[error("notification not found")]
    NotFound,
    #[error("`{0}` is mandatory and can't be turned off")]
    Mandatory(&'static str),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

pub type NotifyResult<T> = Result<T, NotifyError>;

/// What to tell the user.
#[derive(Debug, Clone)]
pub struct Notice<'a> {
    pub user_id: Uuid,
    pub kind: &'static str,
    pub title: &'a str,
    pub body: &'a str,
    /// A path in the staff app, e.g. `/profile`.
    pub link: Option<&'a str>,
}

/// The channels `user_id` wants for `kind`.
async fn channels(
    conn: &mut PgConnection,
    user_id: Uuid,
    kind: &Kind,
) -> NotifyResult<(bool, bool)> {
    if kind.mandatory {
        return Ok((true, true));
    }
    let pref: Option<(bool, bool)> = sqlx::query_as(
        "SELECT in_app, email FROM notification_preferences WHERE user_id = $1 AND kind = $2",
    )
    .bind(user_id)
    .bind(kind.key)
    .fetch_optional(conn)
    .await?;
    Ok(pref.unwrap_or((kind.in_app, kind.email)))
}

/// Records a notification in the caller's transaction. Service accounts are
/// skipped (nobody reads their notifications). Returns its id, if recorded.
pub async fn notify(conn: &mut PgConnection, notice: Notice<'_>) -> NotifyResult<Option<Uuid>> {
    let kind =
        kinds::get(notice.kind).ok_or_else(|| NotifyError::UnknownKind(notice.kind.into()))?;
    let human: Option<(bool,)> =
        sqlx::query_as("SELECT kind = 'human' FROM users WHERE id = $1 AND status = 'active'")
            .bind(notice.user_id)
            .fetch_optional(&mut *conn)
            .await?;
    if !matches!(human, Some((true,))) {
        return Ok(None);
    }
    let (in_app, email) = channels(conn, notice.user_id, kind).await?;
    if !in_app && !email {
        return Ok(None);
    }
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, title, body, link, in_app, email_status, email_next_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::notification_email_status, CASE WHEN $8 = 'pending' THEN now() END)",
    )
    .bind(id)
    .bind(notice.user_id)
    .bind(kind.key)
    .bind(notice.title)
    .bind(notice.body)
    .bind(notice.link)
    .bind(in_app)
    .bind(if email { "pending" } else { "skipped" })
    .execute(&mut *conn)
    .await?;
    if in_app {
        signal(conn, notice.user_id).await?;
    }
    Ok(Some(id))
}

/// [`notify`] for every member of a team.
pub async fn notify_team(
    conn: &mut PgConnection,
    team_id: Uuid,
    kind: &'static str,
    title: &str,
    body: &str,
    link: Option<&str>,
) -> NotifyResult<()> {
    let members: Vec<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM team_members WHERE team_id = $1")
            .bind(team_id)
            .fetch_all(&mut *conn)
            .await?;
    for (user_id,) in members {
        notify(
            conn,
            Notice {
                user_id,
                kind,
                title,
                body,
                link,
            },
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// The user's in-app notifications, newest first, before `before` if given.
/// Returns up to `limit + 1` so callers can tell whether more exist.
pub async fn list(
    conn: &mut PgConnection,
    user_id: Uuid,
    before: Option<Uuid>,
    limit: u32,
    unread_only: bool,
) -> NotifyResult<Vec<Notification>> {
    Ok(sqlx::query_as(
        "SELECT id, kind, title, body, link, read_at, created_at FROM notifications \
          WHERE user_id = $1 AND in_app AND ($2::uuid IS NULL OR id < $2) \
            AND (NOT $4 OR read_at IS NULL) \
          ORDER BY id DESC LIMIT $3",
    )
    .bind(user_id)
    .bind(before)
    .bind(i64::from(limit) + 1)
    .bind(unread_only)
    .fetch_all(conn)
    .await?)
}

pub async fn unread_count(conn: &mut PgConnection, user_id: Uuid) -> NotifyResult<i64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND in_app AND read_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(conn)
    .await?;
    Ok(count)
}

pub async fn mark_read(conn: &mut PgConnection, user_id: Uuid, id: Uuid) -> NotifyResult<()> {
    let found = sqlx::query(
        "UPDATE notifications SET read_at = coalesce(read_at, now()) WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if found == 0 {
        return Err(NotifyError::NotFound);
    }
    signal(conn, user_id).await
}

/// Tells the user's open sessions their unread count changed.
async fn signal(conn: &mut PgConnection, user_id: Uuid) -> NotifyResult<()> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CHANNEL)
        .bind(user_id.to_string())
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn mark_all_read(conn: &mut PgConnection, user_id: Uuid) -> NotifyResult<u64> {
    let marked = sqlx::query(
        "UPDATE notifications SET read_at = now() WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    signal(conn, user_id).await?;
    Ok(marked)
}

/// A kind with the user's effective channels.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preference {
    pub kind: &'static str,
    /// The module the kind belongs to, if any.
    #[serde(skip)]
    pub module: Option<&'static str>,
    pub description: &'static str,
    pub in_app: bool,
    pub email: bool,
    pub mandatory: bool,
}

pub async fn preferences(conn: &mut PgConnection, user_id: Uuid) -> NotifyResult<Vec<Preference>> {
    let mut out = Vec::with_capacity(kinds::ALL.len());
    for kind in kinds::ALL {
        let (in_app, email) = channels(conn, user_id, kind).await?;
        out.push(Preference {
            kind: kind.key,
            module: kind.module,
            description: kind.description,
            in_app,
            email,
            mandatory: kind.mandatory,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Channels {
    pub in_app: bool,
    pub email: bool,
}

pub async fn set_preference(
    conn: &mut PgConnection,
    user_id: Uuid,
    kind: &str,
    channels: Channels,
) -> NotifyResult<()> {
    let kind = kinds::get(kind).ok_or_else(|| NotifyError::UnknownKind(kind.into()))?;
    if kind.mandatory {
        return Err(NotifyError::Mandatory(kind.key));
    }
    sqlx::query(
        "INSERT INTO notification_preferences (user_id, kind, in_app, email) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, kind) DO UPDATE SET in_app = excluded.in_app, email = excluded.email",
    )
    .bind(user_id)
    .bind(kind.key)
    .bind(channels.in_app)
    .bind(channels.email)
    .execute(conn)
    .await?;
    Ok(())
}

/// How many times an email is tried before it's marked failed.
const MAX_EMAIL_ATTEMPTS: i32 = 5;

/// Sends queued notification emails until `shutdown`. Several workers can run
/// at once: each claims rows with `FOR UPDATE SKIP LOCKED`.
pub async fn run_email_dispatcher(
    db: PgPool,
    mailer: Mailer,
    public_url: Option<String>,
    shutdown: CancellationToken,
) {
    if !mailer.enabled() {
        tracing::info!("email isn't configured; notification emails stay queued as skipped");
    }
    loop {
        if mailer.enabled() {
            match send_batch(&db, &mailer, public_url.as_deref()).await {
                Ok(sent) if sent > 0 => tracing::debug!(sent, "sent notification emails"),
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "notification email dispatch failed"),
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(StdDuration::from_secs(5)) => {}
        }
    }
}

#[derive(sqlx::FromRow)]
struct QueuedEmail {
    id: Uuid,
    recipient: String,
    name: String,
    title: String,
    body: String,
    link: Option<String>,
    attempts: i32,
}

/// Sends one batch. Returns how many were sent.
pub async fn send_batch(
    db: &PgPool,
    mailer: &Mailer,
    public_url: Option<&str>,
) -> NotifyResult<usize> {
    let mut tx = db.begin().await?;
    let due: Vec<QueuedEmail> = sqlx::query_as(
        "SELECT n.id, u.email AS recipient, u.display_name AS name, n.title, n.body, n.link, \
                n.email_attempts AS attempts \
           FROM notifications n JOIN users u ON u.id = n.user_id \
          WHERE n.email_status = 'pending' AND n.email_next_at <= now() \
          ORDER BY n.email_next_at LIMIT 20 \
          FOR UPDATE OF n SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut sent = 0;
    for QueuedEmail {
        id,
        recipient: to,
        name,
        title,
        body,
        link,
        attempts,
    } in due
    {
        let url = match (public_url, &link) {
            (Some(base), Some(path)) => format!("{}{path}", base.trim_end_matches('/')),
            (Some(base), None) => base.trim_end_matches('/').to_owned(),
            (None, _) => String::new(),
        };
        let email = action_email(
            &to,
            &title,
            &format!("Hi {name},"),
            if body.is_empty() { &title } else { &body },
            "Open Huxley",
            &url,
            "You're getting this because of your notification settings in Huxley. Security notices are always sent.",
        );
        match mailer.send(email).await {
            Ok(()) => {
                sqlx::query("UPDATE notifications SET email_status = 'sent', email_attempts = email_attempts + 1, email_error = NULL WHERE id = $1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                sent += 1;
            }
            Err(error) => {
                let attempts = attempts + 1;
                let failed = attempts >= MAX_EMAIL_ATTEMPTS;
                // 1, 2, 4, 8 minutes between tries.
                let backoff = Duration::minutes(1_i64 << (attempts - 1).clamp(0, 4));
                sqlx::query(
                    "UPDATE notifications SET email_attempts = $2, email_error = $3, \
                            email_status = CASE WHEN $4 THEN 'failed' ELSE 'pending' END::notification_email_status, \
                            email_next_at = CASE WHEN $4 THEN NULL ELSE now() + $5 END \
                      WHERE id = $1",
                )
                .bind(id)
                .bind(attempts)
                .bind(error.to_string())
                .bind(failed)
                .bind(backoff)
                .execute(&mut *tx)
                .await?;
                tracing::warn!(%error, attempts, failed, "notification email failed");
            }
        }
    }
    tx.commit().await?;
    Ok(sent)
}

/// Fans `pg_notify` signals out to this process's real-time subscribers.
/// One Postgres connection per API process, whatever the number of clients.
#[derive(Debug, Clone)]
pub struct Broadcaster {
    sender: broadcast::Sender<Uuid>,
    channel: &'static str,
}

impl Broadcaster {
    /// Users with new notifications, from [`CHANNEL`].
    pub fn new() -> Self {
        Self::for_channel(CHANNEL)
    }

    /// Ids signalled on another `pg_notify` channel.
    pub fn for_channel(channel: &'static str) -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender, channel }
    }

    /// Receives the ids signalled on the channel.
    pub fn subscribe(&self) -> broadcast::Receiver<Uuid> {
        self.sender.subscribe()
    }

    /// Listens on the channel until `shutdown`, reconnecting after errors.
    pub async fn run(self, db: PgPool, shutdown: CancellationToken) {
        loop {
            let listen = async {
                let mut listener = PgListener::connect_with(&db).await?;
                listener.listen(self.channel).await?;
                tracing::info!(channel = self.channel, "listening");
                loop {
                    let message = listener.recv().await?;
                    if let Ok(id) = message.payload().parse::<Uuid>() {
                        // No subscribers is fine.
                        let _ = self.sender.send(id);
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), sqlx::Error>(())
            };
            tokio::select! {
                () = shutdown.cancelled() => return,
                result = listen => {
                    if let Err(error) = result {
                        tracing::warn!(%error, channel = self.channel, "listener stopped; reconnecting");
                    }
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(StdDuration::from_secs(2)) => {}
            }
        }
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}
