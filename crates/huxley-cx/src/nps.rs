//! Net promoter surveys (design doc §8.9, ADR-043): contacts who raised a
//! ticket lately are asked, at most every so often, how likely they are to
//! recommend the MSP, 0 to 10. Promoters answer 9 or 10, passives 7 or 8,
//! detractors 0 to 6; the score is promoters' share minus detractors'.

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
pub const TOKEN_PREFIX: &str = "hxnp_";
/// How long a question can be answered.
pub const ANSWER_WINDOW: Duration = Duration::days(30);

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = NpsSurvey)]
pub struct Survey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_name: String,
    pub contact_id: Uuid,
    pub campaign: String,
    pub score: Option<i16>,
    pub comment: Option<String>,
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
    pub fn open(&self) -> bool {
        self.responded_at.is_none() && self.expires_at > OffsetDateTime::now_utc()
    }
}

/// `promoter` (9–10), `passive` (7–8) or `detractor` (0–6).
pub fn category(score: i16) -> &'static str {
    match score {
        9.. => "promoter",
        7 | 8 => "passive",
        _ => "detractor",
    }
}

const SELECT: &str = "SELECT n.id, n.org_id, o.name AS org_name, n.contact_id, n.campaign, n.score, n.comment, \
    n.channel, n.emailed, n.sent_at, n.responded_at, n.expires_at \
    FROM cx_nps n JOIN organizations o ON o.id = n.org_id";

/// Someone to ask.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Candidate {
    pub org_id: Uuid,
    pub contact_id: Uuid,
    pub email: String,
    pub first_name: String,
}

async fn setting(conn: &mut PgConnection, org: Uuid, key: &str) -> CxResult<Option<String>> {
    settings::value(conn, org, key)
        .await
        .map_err(|e| CxError::Db(sqlx::Error::Protocol(e.to_string())))
}

/// Contacts to ask, in `org_id` and the companies below it: those with an
/// email address who raised a ticket lately and weren't asked lately (each
/// company's settings), unless their company opted out. At most `limit`.
pub async fn candidates(
    conn: &mut PgConnection,
    org_id: Uuid,
    limit: i64,
) -> CxResult<Vec<Candidate>> {
    let companies: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT c.org_id FROM crm_contacts c JOIN organizations o ON o.id = c.org_id \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) AND c.archived_at IS NULL",
    )
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for company in companies {
        let taken = i64::try_from(out.len()).unwrap_or(i64::MAX);
        if taken >= limit {
            break;
        }
        if setting(&mut *conn, company, settings::NPS_ENABLED)
            .await?
            .as_deref()
            == Some("false")
        {
            continue;
        }
        let days = async |conn: &mut PgConnection, key: &str, default: i32| -> CxResult<i32> {
            Ok(setting(conn, company, key)
                .await?
                .and_then(|v| v.parse().ok())
                .unwrap_or(default))
        };
        let active = days(&mut *conn, settings::NPS_ACTIVE, 180).await?;
        let interval = days(&mut *conn, settings::NPS_INTERVAL, 90).await?;
        let found: Vec<Candidate> = sqlx::query_as(
            "SELECT c.org_id, c.id AS contact_id, c.email, c.first_name FROM crm_contacts c \
              WHERE c.org_id = $1 AND c.archived_at IS NULL AND c.email LIKE '%@%' \
                AND EXISTS (SELECT 1 FROM sd_tickets t WHERE t.requester_id = c.id \
                               AND t.created_at > now() - make_interval(days => $2)) \
                AND NOT EXISTS (SELECT 1 FROM cx_nps n WHERE n.contact_id = c.id \
                                   AND n.sent_at > now() - make_interval(days => $3)) \
              ORDER BY c.id LIMIT $4",
        )
        .bind(company)
        .bind(active)
        .bind(interval)
        .bind(limit - taken)
        .fetch_all(&mut *conn)
        .await?;
        out.extend(found);
    }
    Ok(out)
}

/// Starts a question for a candidate: the survey and its answer link's token.
pub async fn start(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    who: &Candidate,
    campaign: &str,
) -> CxResult<(Survey, String)> {
    let t = token::generate(TOKEN_PREFIX);
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO cx_nps (id, org_id, contact_id, campaign, token_hash, expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(who.org_id)
    .bind(who.contact_id)
    .bind(campaign)
    .bind(&t.hash)
    .bind(OffsetDateTime::now_utc() + ANSWER_WINDOW)
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("nps.asked")
            .org(who.org_id)
            .target("contact", who.contact_id)
            .details(json!({ "nps": id, "campaign": campaign })),
    )
    .await?;
    Ok((get(conn, id).await?, t.secret))
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CxResult<Survey> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE n.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(CxError::NotFound)
}

pub async fn mark_emailed(conn: &mut PgConnection, id: Uuid) -> CxResult<()> {
    sqlx::query("UPDATE cx_nps SET emailed = true WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// The question an answer link is for.
pub async fn by_token(conn: &mut PgConnection, secret: &str) -> CxResult<Survey> {
    if !token::looks_valid(secret, TOKEN_PREFIX) {
        return Err(CxError::NotFound);
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE n.token_hash = $1"
    )))
    .bind(token::hash(secret))
    .fetch_optional(conn)
    .await?
    .ok_or(CxError::NotFound)
}

/// A contact's question still waiting for an answer, if any.
pub async fn open_for_contact(conn: &mut PgConnection, contact: Uuid) -> CxResult<Option<Survey>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE n.contact_id = $1 AND n.responded_at IS NULL AND n.expires_at > now() \
          ORDER BY n.sent_at DESC LIMIT 1"
    )))
    .bind(contact)
    .fetch_optional(conn)
    .await?)
}

/// A contact's latest question, answered or not.
pub async fn latest_for_contact(
    conn: &mut PgConnection,
    contact: Uuid,
) -> CxResult<Option<Survey>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE n.contact_id = $1 ORDER BY n.sent_at DESC LIMIT 1"
    )))
    .bind(contact)
    .fetch_optional(conn)
    .await?)
}

/// Records the answer (once): 0 to 10, and an optional comment. Emits
/// `nps.responded`.
pub async fn respond(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    survey: &Survey,
    score: i16,
    comment: Option<&str>,
    channel: &str,
) -> CxResult<Survey> {
    if !(0..=10).contains(&score) {
        return Err(CxError::invalid("score", "0 to 10"));
    }
    let comment = comment.map(str::trim).filter(|c| !c.is_empty());
    if comment.is_some_and(|c| c.chars().count() > 4000) {
        return Err(CxError::invalid("comment", "at most 4,000 characters"));
    }
    let answered = sqlx::query(
        "UPDATE cx_nps SET score = $2, comment = $3, channel = $4, responded_at = now() \
          WHERE id = $1 AND responded_at IS NULL AND expires_at > now()",
    )
    .bind(survey.id)
    .bind(score)
    .bind(comment)
    .bind(channel)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if answered == 0 {
        return Err(CxError::Closed);
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("nps.responded")
            .org(survey.org_id)
            .target("contact", survey.contact_id)
            .details(json!({ "nps": survey.id, "score": score, "channel": channel })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "nps.responded",
            org_id: Some(survey.org_id),
            subject_type: "contact",
            subject_id: survey.contact_id,
            payload: json!({
                "nps": survey.id, "campaign": survey.campaign, "score": score,
                "category": category(score), "comment": comment, "channel": channel,
            }),
        },
    )
    .await?;
    get(conn, survey.id).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn categories() {
        assert_eq!(
            [0, 6, 7, 8, 9, 10].map(super::category),
            [
                "detractor",
                "detractor",
                "passive",
                "passive",
                "promoter",
                "promoter"
            ]
        );
    }
}
