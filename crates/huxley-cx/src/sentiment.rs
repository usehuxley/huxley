//! Sentiment (ADR-046): each client message's tone, -2 to 2, with a
//! one-line reason. A workflow judges it (the AI sentiment pack runs
//! `ai.extract`) and `core.sentiment.record` stores it here. It's a client
//! health factor, and a falling 30-day trend flags the client.

use serde::Serialize;
use sqlx::PgConnection;
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CxError, CxResult};

/// The fall in average (30 days against the 60 before) that flags a client.
pub const FALL: f64 = 0.75;
/// Messages needed in each window to judge a trend.
pub const ENOUGH: i64 = 3;

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SentimentEntry)]
pub struct Entry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub ticket_id: Uuid,
    pub note_id: Option<Uuid>,
    pub score: i16,
    pub reason: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Records a message's sentiment: a note's, or (without `note_id`) the
/// ticket's description. Again for the same message replaces it.
pub async fn record(
    conn: &mut PgConnection,
    ticket_id: Uuid,
    note_id: Option<Uuid>,
    score: i64,
    reason: &str,
) -> CxResult<Entry> {
    let score = i16::try_from(score)
        .ok()
        .filter(|s| (-2..=2).contains(s))
        .ok_or_else(|| CxError::invalid("score", "-2 (very negative) to 2 (very positive)"))?;
    let reason: String = reason.trim().chars().take(300).collect();
    if reason.is_empty() {
        return Err(CxError::invalid("reason", "say why, in a line"));
    }
    let found: Option<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT org_id, requester_id FROM sd_tickets WHERE id = $1")
            .bind(ticket_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (org_id, requester) = found.ok_or(CxError::NotFound)?;
    let contact = match note_id {
        Some(note) => {
            let row: Option<Option<Uuid>> = sqlx::query_scalar(
                "SELECT author_contact_id FROM sd_ticket_notes WHERE id = $1 AND ticket_id = $2",
            )
            .bind(note)
            .bind(ticket_id)
            .fetch_optional(&mut *conn)
            .await?;
            row.ok_or_else(|| CxError::invalid("note_id", "not a note on that ticket"))?
        }
        None => requester,
    };
    let conflict = if note_id.is_some() {
        "(note_id) WHERE note_id IS NOT NULL"
    } else {
        "(ticket_id) WHERE note_id IS NULL"
    };
    let entry: Entry = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO cx_sentiment (id, org_id, ticket_id, note_id, contact_id, score, reason) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT {conflict} DO UPDATE SET score = EXCLUDED.score, reason = EXCLUDED.reason, at = now() \
         RETURNING id, org_id, ticket_id, note_id, score, reason, at"
    )))
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(ticket_id)
    .bind(note_id)
    .bind(contact)
    .bind(score)
    .bind(&reason)
    .fetch_one(conn)
    .await?;
    Ok(entry)
}

/// A ticket's judged messages, oldest first.
pub async fn for_ticket(conn: &mut PgConnection, ticket_id: Uuid) -> CxResult<Vec<Entry>> {
    Ok(sqlx::query_as(
        "SELECT id, org_id, ticket_id, note_id, score, reason, at FROM cx_sentiment WHERE ticket_id = $1 ORDER BY at",
    )
    .bind(ticket_id)
    .fetch_all(conn)
    .await?)
}

/// A company's sentiment over the last 30 days against the 60 before.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    pub recent: Option<f64>,
    pub recent_n: i64,
    pub before: Option<f64>,
    pub before_n: i64,
    /// The most negative recent messages: "T-123: reason", worst first.
    pub worst: Vec<String>,
}

impl Trend {
    /// Fell by [`FALL`] or more, with enough messages either side.
    pub fn falling(&self) -> bool {
        match (self.recent, self.before) {
            (Some(r), Some(b)) => {
                self.recent_n >= ENOUGH && self.before_n >= ENOUGH && b - r >= FALL
            }
            _ => false,
        }
    }
}

pub async fn trend(conn: &mut PgConnection, company: Uuid, today: Date) -> CxResult<Trend> {
    let recent_from = (today - Duration::days(30)).midnight().assume_utc();
    let before_from = (today - Duration::days(90)).midnight().assume_utc();
    let (recent, recent_n, before, before_n): (Option<f64>, i64, Option<f64>, i64) =
        sqlx::query_as(
            "SELECT avg(score) FILTER (WHERE at >= $2)::float8, count(*) FILTER (WHERE at >= $2), \
                avg(score) FILTER (WHERE at < $2)::float8, count(*) FILTER (WHERE at < $2) \
           FROM cx_sentiment WHERE org_id = $1 AND at >= $3",
        )
        .bind(company)
        .bind(recent_from)
        .bind(before_from)
        .fetch_one(&mut *conn)
        .await?;
    let worst: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.number_prefix || '-' || t.number, s.reason FROM cx_sentiment s JOIN sd_tickets t ON t.id = s.ticket_id \
          WHERE s.org_id = $1 AND s.at >= $2 AND s.score < 0 ORDER BY s.score, s.at DESC LIMIT 3",
    )
    .bind(company)
    .bind(recent_from)
    .fetch_all(conn)
    .await?;
    Ok(Trend {
        recent,
        recent_n,
        before,
        before_n,
        worst: worst
            .into_iter()
            .map(|(r, why)| format!("{r}: {why}"))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trend_falls_with_enough_messages() {
        let t = |recent, recent_n, before, before_n| Trend {
            recent,
            recent_n,
            before,
            before_n,
            worst: vec![],
        };
        assert!(t(Some(-1.0), 3, Some(0.5), 4).falling());
        assert!(!t(Some(-1.0), 2, Some(0.5), 4).falling());
        assert!(!t(Some(0.0), 5, Some(0.5), 5).falling());
        assert!(!t(None, 0, Some(0.5), 5).falling());
    }
}
