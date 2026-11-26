//! Calendar sync (ADR-061, design doc §8.15): people's busy time from their
//! work calendars, so dispatch, the dispatch board and capacity planning
//! know when they're actually free.
//!
//! Microsoft 365 is reached through an `msgraph` connection (its free/busy
//! schedule, so private events stay private); anything else through an
//! iCalendar feed URL. A sync (the `core.calendar.sync` step, which the
//! built-in pack runs every few minutes) replaces each person's busy blocks
//! for a window around now. Blocks are free/busy only unless subjects are
//! kept, and never a private event's.

use huxley_core::RequestContext;
use huxley_crypto::Binding;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkforceError, WorkforceResult};

pub const PROVIDERS: &[&str] = &["m365"];
pub const SOURCES: &[&str] = &["provider", "ics", "none"];
/// Synced from a day back to this many days ahead.
pub const WINDOW_DAYS: i64 = 60;

/// What a block of time is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    Busy,
    Tentative,
    /// Out of office.
    Oof,
    /// Working elsewhere.
    Elsewhere,
}

impl BlockStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Tentative => "tentative",
            Self::Oof => "oof",
            Self::Elsewhere => "elsewhere",
        }
    }

    /// From a Microsoft Graph schedule item's status (`free` isn't a block).
    pub fn from_graph(status: &str) -> Option<Self> {
        match status {
            "busy" => Some(Self::Busy),
            "tentative" => Some(Self::Tentative),
            "oof" => Some(Self::Oof),
            "workingElsewhere" => Some(Self::Elsewhere),
            _ => None,
        }
    }
}

/// A block of busy time (unix seconds), as read from a calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub starts_at: i64,
    pub ends_at: i64,
    pub status: BlockStatus,
    pub subject: Option<String>,
}

/// A stored block of busy time.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BusyBlock {
    pub user_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
    /// `busy`, `tentative`, `oof` (out of office) or `elsewhere`.
    pub status: String,
    /// Only when subjects are kept, and never a private event's.
    pub subject: Option<String>,
}

/// How calendars are reached.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = CalendarSettings)]
pub struct Settings {
    /// `m365`.
    pub provider: String,
    /// The organization the connection is at.
    pub org_id: Uuid,
    /// The `msgraph` connection's key.
    pub connection: String,
    pub keep_subjects: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = CalendarSettingsInput)]
pub struct SettingsInput {
    pub provider: String,
    pub org_id: Uuid,
    pub connection: String,
    #[serde(default)]
    pub keep_subjects: bool,
}

pub async fn settings(conn: &mut PgConnection) -> WorkforceResult<Option<Settings>> {
    Ok(sqlx::query_as(
        "SELECT provider, org_id, connection, keep_subjects, updated_at FROM wf_calendar_settings",
    )
    .fetch_optional(conn)
    .await?)
}

pub async fn set_settings(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &SettingsInput,
) -> WorkforceResult<Settings> {
    if !PROVIDERS.contains(&input.provider.as_str()) {
        return Err(WorkforceError::invalid("provider", "m365"));
    }
    let connection = input.connection.trim();
    let valid = connection.len() <= 64
        && connection.starts_with(|c: char| c.is_ascii_lowercase())
        && connection
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        return Err(WorkforceError::invalid("connection", "a connection's key"));
    }
    sqlx::query(
        "INSERT INTO wf_calendar_settings (id, provider, org_id, connection, keep_subjects, updated_by) \
         VALUES (true, $1, $2, $3, $4, $5) \
         ON CONFLICT (id) DO UPDATE SET provider = $1, org_id = $2, connection = $3, keep_subjects = $4, \
                updated_by = $5, updated_at = now()",
    )
    .bind(&input.provider)
    .bind(input.org_id)
    .bind(connection)
    .bind(input.keep_subjects)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if !input.keep_subjects {
        sqlx::query("UPDATE wf_busy_blocks SET subject = NULL WHERE subject IS NOT NULL")
            .execute(&mut *conn)
            .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("calendar.settings_changed").details(json!({
            "provider": input.provider, "connection": connection, "keepSubjects": input.keep_subjects,
        })),
    )
    .await?;
    settings(conn).await?.ok_or(WorkforceError::NotFound)
}

/// Stops syncing from the provider (feeds keep syncing).
pub async fn clear_settings(conn: &mut PgConnection, ctx: &RequestContext) -> WorkforceResult<()> {
    sqlx::query("DELETE FROM wf_calendar_settings")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "DELETE FROM wf_busy_blocks b WHERE NOT EXISTS (SELECT 1 FROM wf_calendar_people p \
          WHERE p.user_id = b.user_id AND p.source = 'ics')",
    )
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("calendar.settings_cleared"),
    )
    .await?;
    Ok(())
}

/// Someone whose calendar can be synced, and how it is.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CalendarPerson {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    /// `provider`, `ics` or `none`.
    pub source: String,
    /// Their calendar's address at the provider, when not their email.
    pub calendar: Option<String>,
    /// An ICS feed is set (its URL is never shown).
    pub has_feed: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_synced_at: Option<OffsetDateTime>,
    pub last_error: Option<String>,
}

/// Everyone active, and how their calendar is synced (no row: from the
/// provider, at their email).
pub async fn people(conn: &mut PgConnection) -> WorkforceResult<Vec<CalendarPerson>> {
    Ok(sqlx::query_as(
        "SELECT u.id AS user_id, u.display_name AS name, u.email, coalesce(p.source, 'provider') AS source, \
                p.calendar, p.ics_url IS NOT NULL AS has_feed, p.last_synced_at, p.last_error \
           FROM users u LEFT JOIN wf_calendar_people p ON p.user_id = u.id \
          WHERE u.status = 'active' AND u.kind = 'human' ORDER BY lower(u.display_name)",
    )
    .fetch_all(conn)
    .await?)
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarPersonInput {
    /// `provider`, `ics` or `none`.
    pub source: String,
    /// Their calendar's address at the provider, when not their email.
    #[serde(default)]
    pub calendar: Option<String>,
    /// A new ICS feed URL (never returned). Leave out to keep the current one.
    #[serde(default)]
    pub ics_url: Option<String>,
}

/// The binding an ICS feed URL is sealed with.
pub const fn ics_binding(user: Uuid) -> Binding<'static> {
    Binding {
        purpose: "calendar.ics_url",
        record: user,
    }
}

/// Checks an ICS feed URL: http(s), at most 2,000 characters.
pub fn check_feed(url: &str) -> WorkforceResult<()> {
    let url = url.trim();
    let scheme_ok =
        url.starts_with("https://") || url.starts_with("http://") || url.starts_with("webcal://");
    if !scheme_ok || url.len() > 2000 || url.contains(char::is_whitespace) {
        return Err(WorkforceError::invalid(
            "icsUrl",
            "an https:// (or webcal://) address",
        ));
    }
    Ok(())
}

/// Sets how someone's calendar is synced; `sealed` is a new feed URL,
/// sealed with [`ics_binding`] (`None` keeps the current one).
pub async fn set_person(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    input: &CalendarPersonInput,
    sealed: Option<Vec<u8>>,
) -> WorkforceResult<CalendarPerson> {
    if !SOURCES.contains(&input.source.as_str()) {
        return Err(WorkforceError::invalid("source", "provider, ics or none"));
    }
    let calendar = input
        .calendar
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    if calendar.is_some_and(|c| c.len() > 320 || !c.contains('@')) {
        return Err(WorkforceError::invalid("calendar", "an email address"));
    }
    let has_feed: Option<bool> =
        sqlx::query_scalar("SELECT ics_url IS NOT NULL FROM wf_calendar_people WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&mut *conn)
            .await?;
    if input.source == "ics" && sealed.is_none() && has_feed != Some(true) {
        return Err(WorkforceError::invalid("icsUrl", "a feed URL is needed"));
    }
    sqlx::query(
        "INSERT INTO wf_calendar_people (user_id, source, calendar, ics_url) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id) DO UPDATE SET source = $2, calendar = $3, \
                ics_url = coalesce($4, wf_calendar_people.ics_url), last_error = NULL, updated_at = now()",
    )
    .bind(user)
    .bind(&input.source)
    .bind(calendar)
    .bind(sealed)
    .execute(&mut *conn)
    .await?;
    if input.source == "none" {
        sqlx::query("DELETE FROM wf_busy_blocks WHERE user_id = $1")
            .bind(user)
            .execute(&mut *conn)
            .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("calendar.person_changed")
            .target("user", user)
            .details(json!({ "source": input.source })),
    )
    .await?;
    people(conn)
        .await?
        .into_iter()
        .find(|p| p.user_id == user)
        .ok_or(WorkforceError::NotFound)
}

/// Someone to sync, with what's needed to reach their calendar.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Target {
    pub user_id: Uuid,
    /// Their calendar address at the provider (theirs, or their email).
    pub address: String,
    /// `provider` or `ics`.
    pub source: String,
    pub ics_url: Option<Vec<u8>>,
    /// Their working hours' time zone (for times without one).
    pub timezone: String,
}

/// Everyone whose calendar is synced: from the provider (when it's set up)
/// or from a feed.
pub async fn targets(conn: &mut PgConnection, provider: bool) -> WorkforceResult<Vec<Target>> {
    Ok(sqlx::query_as(
        "SELECT u.id AS user_id, coalesce(p.calendar, u.email) AS address, coalesce(p.source, 'provider') AS source, \
                p.ics_url, coalesce(s.timezone, 'UTC') AS timezone \
           FROM users u LEFT JOIN wf_calendar_people p ON p.user_id = u.id \
           LEFT JOIN wf_schedules s ON s.user_id = u.id \
          WHERE u.status = 'active' AND u.kind = 'human' \
            AND (coalesce(p.source, 'provider') = 'ics' OR ($1 AND coalesce(p.source, 'provider') = 'provider')) \
          ORDER BY u.id",
    )
    .bind(provider)
    .fetch_all(conn)
    .await?)
}

/// Replaces someone's busy blocks in `[from, to)` with those read.
pub async fn replace(
    conn: &mut PgConnection,
    user: Uuid,
    (from, to): (i64, i64),
    blocks: &[Block],
    keep_subjects: bool,
) -> WorkforceResult<usize> {
    let ts = |s: i64| {
        OffsetDateTime::from_unix_timestamp(s)
            .map_err(|_| WorkforceError::invalid("time", "out of range"))
    };
    let (from_at, to_at) = (ts(from)?, ts(to)?);
    sqlx::query(
        "DELETE FROM wf_busy_blocks WHERE user_id = $1 AND starts_at < $3 AND ends_at > $2",
    )
    .bind(user)
    .bind(from_at)
    .bind(to_at)
    .execute(&mut *conn)
    .await?;
    let mut stored = 0;
    for b in blocks
        .iter()
        .filter(|b| b.ends_at > from && b.starts_at < to && b.ends_at > b.starts_at)
    {
        sqlx::query(
            "INSERT INTO wf_busy_blocks (id, user_id, starts_at, ends_at, status, subject) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(user)
        .bind(ts(b.starts_at)?)
        .bind(ts(b.ends_at)?)
        .bind(b.status.as_str())
        .bind(if keep_subjects {
            b.subject.as_deref()
        } else {
            None
        })
        .execute(&mut *conn)
        .await?;
        stored += 1;
    }
    Ok(stored)
}

/// Records how someone's sync went.
pub async fn record(
    conn: &mut PgConnection,
    user: Uuid,
    error: Option<&str>,
) -> WorkforceResult<()> {
    sqlx::query(
        "INSERT INTO wf_calendar_people (user_id, last_synced_at, last_error) \
         VALUES ($1, CASE WHEN $2::text IS NULL THEN now() END, $2) \
         ON CONFLICT (user_id) DO UPDATE SET \
                last_synced_at = CASE WHEN $2::text IS NULL THEN now() ELSE wf_calendar_people.last_synced_at END, \
                last_error = $2",
    )
    .bind(user)
    .bind(error.map(|e| e.chars().take(500).collect::<String>()))
    .execute(conn)
    .await?;
    Ok(())
}

/// Busy blocks of these people overlapping `[from, to)`, by start.
pub async fn between(
    conn: &mut PgConnection,
    people: &[Uuid],
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> WorkforceResult<Vec<BusyBlock>> {
    Ok(sqlx::query_as(
        "SELECT user_id, starts_at, ends_at, status, subject FROM wf_busy_blocks \
          WHERE user_id = ANY($1) AND starts_at < $3 AND ends_at > $2 ORDER BY starts_at",
    )
    .bind(people)
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?)
}
