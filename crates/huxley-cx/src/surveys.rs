//! CSAT surveys (ADR-043): one per resolution (a reopened and re-resolved
//! ticket can be asked again after a week), sampled, answered once by a
//! one-click email link or in the portal.

use huxley_auth::token;
use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_orgs::settings;
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CxError, CxResult};

/// Answer-link tokens.
pub const TOKEN_PREFIX: &str = "hxcs_";
/// How long a survey can be answered.
pub const ANSWER_WINDOW: Duration = Duration::days(30);
/// Not asked about the same ticket again within this.
const REASK_AFTER: Duration = Duration::days(7);

/// A survey and its answer.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = CsatSurvey)]
pub struct Survey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub ticket_id: Uuid,
    pub reference: String,
    pub title: String,
    pub contact_id: Option<Uuid>,
    /// Who the ticket was assigned to when it was resolved.
    pub assignee_user_id: Option<Uuid>,
    /// `five` (1 to 5) or `thumbs` (1 down, 5 up).
    pub scale: String,
    pub rating: Option<i16>,
    pub comment: Option<String>,
    /// `email` or `portal`.
    pub channel: Option<String>,
    pub emailed: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub sent_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub responded_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl Survey {
    /// Still waiting for an answer.
    pub fn open(&self) -> bool {
        self.responded_at.is_none() && self.expires_at > OffsetDateTime::now_utc()
    }
}

const SELECT: &str = "SELECT s.id, s.org_id, s.ticket_id, t.number_prefix || '-' || t.number AS reference, t.title, \
    s.contact_id, s.assignee_user_id, s.scale, s.rating, s.comment, s.channel, s.emailed, s.sent_at, s.responded_at, s.expires_at \
    FROM cx_surveys s JOIN sd_tickets t ON t.id = s.ticket_id";

/// Who to ask, when a survey was started.
#[derive(Debug, Clone)]
pub struct Recipient {
    pub email: Option<String>,
    pub first_name: String,
}

/// What starting a survey did.
#[derive(Debug, Clone)]
pub enum Started {
    Survey {
        survey: Box<Survey>,
        token: String,
        to: Recipient,
    },
    /// Not asked: `not_resolved`, `opted_out`, `no_requester`,
    /// `already_asked` or `sampled_out`.
    Skipped(&'static str),
}

/// The ticket's company, status category, requester, assignee, and the
/// requester's email and first name.
type TicketRow = (
    Uuid,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
);

async fn setting(conn: &mut PgConnection, org: Uuid, key: &str) -> CxResult<Option<String>> {
    settings::value(conn, org, key)
        .await
        .map_err(|e| CxError::Db(sqlx::Error::Protocol(e.to_string())))
}

/// Starts a survey for a resolved ticket, unless its company opted out,
/// it has no requester, they were asked about it lately, or sampling
/// leaves it out. `roll` is a number in `[0, 1)` (random in production).
pub async fn start(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    roll: f64,
) -> CxResult<Started> {
    let row: Option<TicketRow> = sqlx::query_as(
        "SELECT t.org_id, s.category, t.requester_id, t.assignee_user_id, c.email, c.first_name \
           FROM sd_tickets t JOIN sd_statuses s ON s.id = t.status_id \
           LEFT JOIN crm_contacts c ON c.id = t.requester_id AND c.archived_at IS NULL WHERE t.id = $1",
    )
    .bind(ticket)
    .fetch_optional(&mut *conn)
    .await?;
    let (org, category, requester, assignee, email, first_name) = row.ok_or(CxError::NotFound)?;
    if category != "resolved" {
        return Ok(Started::Skipped("not_resolved"));
    }
    if setting(&mut *conn, org, settings::CSAT_ENABLED)
        .await?
        .as_deref()
        == Some("false")
    {
        return Ok(Started::Skipped("opted_out"));
    }
    let Some(contact) = requester.filter(|_| first_name.is_some()) else {
        return Ok(Started::Skipped("no_requester"));
    };
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cx_surveys WHERE ticket_id = $1 AND sent_at > now() - make_interval(days => $2))",
    )
    .bind(ticket)
    .bind(i32::try_from(REASK_AFTER.whole_days()).unwrap_or(7))
    .fetch_one(&mut *conn)
    .await?;
    if recent {
        return Ok(Started::Skipped("already_asked"));
    }
    let sample: f64 = setting(&mut *conn, org, settings::CSAT_SAMPLE)
        .await?
        .and_then(|s| s.parse().ok())
        .unwrap_or(100.0);
    if roll * 100.0 >= sample {
        return Ok(Started::Skipped("sampled_out"));
    }
    let thumbs = setting(&mut *conn, org, settings::CSAT_THUMBS)
        .await?
        .as_deref()
        == Some("true");
    let t = token::generate(TOKEN_PREFIX);
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO cx_surveys (id, org_id, ticket_id, contact_id, assignee_user_id, scale, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(org)
    .bind(ticket)
    .bind(contact)
    .bind(assignee)
    .bind(if thumbs { "thumbs" } else { "five" })
    .bind(&t.hash)
    .bind(OffsetDateTime::now_utc() + ANSWER_WINDOW)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("csat.survey_started")
            .org(org)
            .target("ticket", ticket)
            .details(json!({ "survey": id })),
    )
    .await?;
    let survey = get(&mut *conn, id).await?;
    Ok(Started::Survey {
        survey: Box::new(survey),
        token: t.secret,
        to: Recipient {
            email: email.filter(|e| e.contains('@')),
            first_name: first_name.unwrap_or_default(),
        },
    })
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CxResult<Survey> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE s.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CxError::NotFound)
}

/// Records that the survey was emailed.
pub async fn mark_emailed(conn: &mut PgConnection, id: Uuid) -> CxResult<()> {
    sqlx::query("UPDATE cx_surveys SET emailed = true WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// The survey an answer link is for (answered or not), if it's real.
pub async fn by_token(conn: &mut PgConnection, secret: &str) -> CxResult<Survey> {
    if !token::looks_valid(secret, TOKEN_PREFIX) {
        return Err(CxError::NotFound);
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.token_hash = $1"
    )))
    .bind(token::hash(secret))
    .fetch_optional(conn)
    .await?
    .ok_or(CxError::NotFound)
}

/// A ticket's latest survey, if any.
pub async fn latest_for_ticket(conn: &mut PgConnection, ticket: Uuid) -> CxResult<Option<Survey>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE s.ticket_id = $1 ORDER BY s.sent_at DESC LIMIT 1"
    )))
    .bind(ticket)
    .fetch_optional(conn)
    .await?)
}

/// Records the answer (once): a rating that fits the scale, and an
/// optional comment. Emits `csat.responded`.
pub async fn respond(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    survey: &Survey,
    rating: i16,
    comment: Option<&str>,
    channel: &str,
) -> CxResult<Survey> {
    let fits = if survey.scale == "thumbs" {
        rating == 1 || rating == 5
    } else {
        (1..=5).contains(&rating)
    };
    if !fits {
        return Err(CxError::invalid(
            "rating",
            if survey.scale == "thumbs" {
                "1 (down) or 5 (up)"
            } else {
                "1 to 5"
            },
        ));
    }
    let comment = comment.map(str::trim).filter(|c| !c.is_empty());
    if comment.is_some_and(|c| c.chars().count() > 4000) {
        return Err(CxError::invalid("comment", "at most 4,000 characters"));
    }
    let answered = sqlx::query(
        "UPDATE cx_surveys SET rating = $2, comment = $3, channel = $4, responded_at = now() \
          WHERE id = $1 AND responded_at IS NULL AND expires_at > now()",
    )
    .bind(survey.id)
    .bind(rating)
    .bind(comment)
    .bind(channel)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if answered == 0 {
        return Err(CxError::Closed);
    }
    let positive = if survey.scale == "thumbs" {
        rating == 5
    } else {
        rating >= 4
    };
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("csat.responded")
            .org(survey.org_id)
            .target("ticket", survey.ticket_id)
            .details(json!({ "survey": survey.id, "rating": rating, "channel": channel })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "csat.responded",
            org_id: Some(survey.org_id),
            subject_type: "ticket",
            subject_id: survey.ticket_id,
            payload: json!({
                "survey": survey.id, "reference": survey.reference, "title": survey.title,
                "rating": rating, "scale": survey.scale, "positive": positive,
                "comment": comment, "channel": channel, "contactId": survey.contact_id,
                "assigneeUserId": survey.assignee_user_id,
            }),
        },
    )
    .await?;
    get(conn, survey.id).await
}
