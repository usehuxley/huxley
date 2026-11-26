//! Client health (design doc §8.9): each client company's score, 0 to 100,
//! a weighted average of factors that have enough data, with the reasons
//! behind it. Computed daily; a drop of [`DECLINE`] points from a week
//! before emits `client.health_declining`.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::{Date, Duration};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CxError, CxResult};

/// A drop of this many points from a week before is "declining".
pub const DECLINE: i16 = 10;
/// Factors under this are the reasons.
const WEAK: f64 = 60.0;

/// The factors, with their default weights (0 turns one off).
pub const FACTORS: &[(&str, &str, u8)] = &[
    ("csat", "Satisfaction surveys", 3),
    ("nps", "Net promoter score", 2),
    ("sentiment", "Client sentiment", 2),
    ("sla", "SLA targets met", 3),
    ("reopens", "Tickets staying resolved", 2),
    ("volume", "Ticket volume", 1),
    ("renewal", "Agreement renewals", 1),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HealthFactor)]
pub struct Factor {
    pub key: String,
    pub label: String,
    /// 0 to 100; none without enough data.
    pub score: Option<f64>,
    /// What it's based on, e.g. "12% of 40 tickets missed an SLA target".
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ClientHealth)]
pub struct Health {
    pub org_id: Uuid,
    pub score: Option<i16>,
    pub factors: Vec<Factor>,
    pub reasons: Vec<String>,
    #[serde(serialize_with = "ymd")]
    #[schema(value_type = String, format = Date)]
    pub computed_on: Date,
    /// Scores over the last 90 days, oldest first.
    pub history: Vec<HealthPoint>,
}

/// A day's score.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct HealthPoint {
    #[serde(serialize_with = "ymd")]
    #[schema(value_type = String, format = Date)]
    pub day: Date,
    pub score: Option<i16>,
}

/// A date as `YYYY-MM-DD` (serde passes it by reference).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn ymd<S: serde::Serializer>(d: &Date, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!(
        "{:04}-{:02}-{:02}",
        d.year(),
        u8::from(d.month()),
        d.day()
    ))
}

/// Each factor's weight, 0 to 10.
pub type Weights = BTreeMap<String, u8>;

pub fn default_weights() -> Weights {
    FACTORS
        .iter()
        .map(|(k, _, w)| ((*k).to_owned(), *w))
        .collect()
}

/// The weights `org_id`'s clients are scored with: the nearest model at it
/// or above, else the defaults.
pub async fn weights(conn: &mut PgConnection, org_id: Uuid) -> CxResult<Weights> {
    let found: Option<sqlx::types::Json<Weights>> = sqlx::query_scalar(
        "SELECT m.weights FROM cx_health_models m JOIN organizations o ON o.id = m.org_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) ORDER BY nlevel(o.path) DESC LIMIT 1",
    )
    .bind(org_id)
    .fetch_optional(conn)
    .await?;
    let mut w = default_weights();
    if let Some(found) = found {
        for (k, v) in found.0 {
            if w.contains_key(&k) {
                w.insert(k, v.min(10));
            }
        }
    }
    Ok(w)
}

/// Sets the weights for `org_id`'s clients (and those below).
pub async fn set_weights(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    given: &Weights,
) -> CxResult<Weights> {
    let mut w = default_weights();
    for (k, v) in given {
        if !w.contains_key(k) {
            return Err(CxError::invalid("weights", format!("{k:?} isn't a factor")));
        }
        if *v > 10 {
            return Err(CxError::invalid("weights", "0 to 10 each"));
        }
        w.insert(k.clone(), *v);
    }
    if w.values().all(|v| *v == 0) {
        return Err(CxError::invalid(
            "weights",
            "at least one factor needs a weight",
        ));
    }
    sqlx::query(
        "INSERT INTO cx_health_models (org_id, weights) VALUES ($1, $2) \
         ON CONFLICT (org_id) DO UPDATE SET weights = EXCLUDED.weights, updated_at = now()",
    )
    .bind(org_id)
    .bind(sqlx::types::Json(&w))
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("health.model_set")
            .org(org_id)
            .details(json!({ "weights": w })),
    )
    .await?;
    Ok(w)
}

fn factor(key: &str, score: Option<f64>, detail: String) -> Factor {
    let label = FACTORS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map_or(key, |(_, l, _)| l);
    Factor {
        key: key.to_owned(),
        label: label.to_owned(),
        score: score.map(|s| s.clamp(0.0, 100.0).round()),
        detail,
    }
}

/// A company's factors as of `today`.
pub async fn factors(conn: &mut PgConnection, company: Uuid, today: Date) -> CxResult<Vec<Factor>> {
    let mut out = Vec::new();
    answers(&mut *conn, company, today, &mut out).await?;
    out.push(sentiment_factor(
        &crate::sentiment::trend(&mut *conn, company, today).await?,
    ));
    desk(&mut *conn, company, today, &mut out).await?;
    renewal(&mut *conn, company, today, &mut out).await?;
    Ok(out)
}

/// Satisfaction and net promoter answers.
async fn answers(
    conn: &mut PgConnection,
    company: Uuid,
    today: Date,
    out: &mut Vec<Factor>,
) -> CxResult<()> {
    let since = |days: i64| today - Duration::days(days);

    let (csat, n): (Option<f64>, i64) = sqlx::query_as(
        "SELECT avg(CASE WHEN (scale = 'thumbs' AND rating = 5) OR (scale = 'five' AND rating >= 4) THEN 100.0 ELSE 0.0 END)::float8, \
                count(*) FROM cx_surveys WHERE org_id = $1 AND responded_at IS NOT NULL AND responded_at >= $2",
    )
    .bind(company)
    .bind(since(90).midnight().assume_utc())
    .fetch_one(&mut *conn)
    .await?;
    out.push(if n >= 3 {
        factor(
            "csat",
            csat,
            format!(
                "{:.0}% satisfied ({n} answers in 90 days)",
                csat.unwrap_or(0.0)
            ),
        )
    } else {
        factor(
            "csat",
            None,
            format!("{n} survey answers in 90 days: too few to tell"),
        )
    });

    let (nps, n): (Option<f64>, i64) = sqlx::query_as(
        "SELECT avg(CASE WHEN score >= 9 THEN 100.0 WHEN score >= 7 THEN 0.0 ELSE -100.0 END)::float8, count(*) \
           FROM cx_nps WHERE org_id = $1 AND responded_at IS NOT NULL AND responded_at >= $2",
    )
    .bind(company)
    .bind(since(180).midnight().assume_utc())
    .fetch_one(&mut *conn)
    .await?;
    out.push(match nps.filter(|_| n >= 3) {
        Some(v) => factor(
            "nps",
            Some(f64::midpoint(v, 100.0)),
            format!("NPS {v:.0} ({n} answers in 180 days)"),
        ),
        None => factor(
            "nps",
            None,
            format!("{n} net promoter answers in 180 days: too few to tell"),
        ),
    });

    Ok(())
}

/// The tone of the client's messages (ADR-046).
fn sentiment_factor(t: &crate::sentiment::Trend) -> Factor {
    use std::fmt::Write as _;
    match t.recent.filter(|_| t.recent_n >= crate::sentiment::ENOUGH) {
        Some(avg) => {
            let mut detail = format!("average {avg:+.1} on {} messages in 30 days", t.recent_n);
            if let Some(before) = t.before.filter(|_| t.before_n >= crate::sentiment::ENOUGH) {
                let _ = write!(detail, ", {before:+.1} the 60 days before");
            }
            if t.falling() && !t.worst.is_empty() {
                let _ = write!(detail, " (e.g. {})", t.worst.join("; "));
            }
            factor("sentiment", Some((avg + 2.0) / 4.0 * 100.0), detail)
        }
        None => factor(
            "sentiment",
            None,
            format!("{} judged messages in 30 days: too few to tell", t.recent_n),
        ),
    }
}

/// SLA misses, reopened tickets and ticket volume.
async fn desk(
    conn: &mut PgConnection,
    company: Uuid,
    today: Date,
    out: &mut Vec<Factor>,
) -> CxResult<()> {
    let since = |days: i64| today - Duration::days(days);
    let (missed, total): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE l.response_breached_at IS NOT NULL OR l.resolution_breached_at IS NOT NULL), count(*) \
           FROM sd_ticket_sla l JOIN sd_tickets t ON t.id = l.ticket_id WHERE t.org_id = $1 AND t.created_at >= $2",
    )
    .bind(company)
    .bind(since(90).midnight().assume_utc())
    .fetch_one(&mut *conn)
    .await?;
    out.push(if total >= 5 {
        #[allow(clippy::cast_precision_loss)]
        let rate = missed as f64 / total as f64;
        factor(
            "sla",
            Some(100.0 * (1.0 - 2.0 * rate)),
            format!(
                "{:.0}% of {total} tickets missed an SLA target",
                rate * 100.0
            ),
        )
    } else {
        factor(
            "sla",
            None,
            format!("{total} tickets with SLA targets in 90 days: too few to tell"),
        )
    });

    let (reopened, resolved): (i64, i64) = sqlx::query_as(
        "SELECT count(DISTINCT t.id) FILTER (WHERE EXISTS ( \
                    SELECT 1 FROM sd_ticket_activity a JOIN sd_statuses s ON s.type_id = t.type_id AND s.key = a.details->>'from' \
                     WHERE a.ticket_id = t.id AND a.kind = 'status' AND s.category = 'resolved' \
                       AND a.details->>'category' NOT IN ('resolved', 'closed'))), \
                count(DISTINCT t.id) \
           FROM sd_tickets t WHERE t.org_id = $1 AND t.resolved_at >= $2",
    )
    .bind(company)
    .bind(since(90).midnight().assume_utc())
    .fetch_one(&mut *conn)
    .await?;
    out.push(if resolved >= 5 {
        #[allow(clippy::cast_precision_loss)]
        let rate = reopened as f64 / resolved as f64;
        factor(
            "reopens",
            Some(100.0 * (1.0 - 3.0 * rate)),
            format!("{reopened} of {resolved} resolved tickets were reopened"),
        )
    } else {
        factor(
            "reopens",
            None,
            format!("{resolved} tickets resolved in 90 days: too few to tell"),
        )
    });

    let (recent, before): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE created_at >= $2), count(*) FILTER (WHERE created_at < $2 AND created_at >= $3) \
           FROM sd_tickets WHERE org_id = $1",
    )
    .bind(company)
    .bind(since(30).midnight().assume_utc())
    .bind(since(120).midnight().assume_utc())
    .fetch_one(&mut *conn)
    .await?;
    #[allow(clippy::cast_precision_loss)]
    let usual = before as f64 / 3.0;
    out.push(if usual >= 3.0 {
        #[allow(clippy::cast_precision_loss)]
        let ratio = recent as f64 / usual;
        let score = if ratio <= 1.25 {
            100.0
        } else {
            100.0 - (ratio - 1.25) / 1.75 * 100.0
        };
        factor(
            "volume",
            Some(score),
            format!("{recent} tickets in 30 days, {ratio:.1}× the usual {usual:.0}"),
        )
    } else {
        factor(
            "volume",
            None,
            format!("{recent} tickets in 30 days; not enough history"),
        )
    });

    Ok(())
}

/// Agreements ending without renewing.
async fn renewal(
    conn: &mut PgConnection,
    company: Uuid,
    today: Date,
    out: &mut Vec<Factor>,
) -> CxResult<()> {
    let (ending, active): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE ends_on IS NOT NULL AND ends_on <= $2 AND NOT auto_renew), count(*) \
           FROM contract_agreements WHERE org_id = $1 AND status = 'active' AND (ends_on IS NULL OR ends_on >= $3)",
    )
    .bind(company)
    .bind(today + Duration::days(60))
    .bind(today)
    .fetch_one(&mut *conn)
    .await?;
    out.push(if active == 0 {
        factor("renewal", None, "No active agreements".to_owned())
    } else if ending > 0 {
        factor(
            "renewal",
            Some(40.0),
            format!("{ending} agreement(s) end within 60 days without renewing automatically"),
        )
    } else {
        factor(
            "renewal",
            Some(100.0),
            format!("{active} active agreement(s), none ending soon"),
        )
    });
    Ok(())
}

/// The weighted score of the factors with data.
pub fn score(factors: &[Factor], weights: &Weights) -> Option<i16> {
    let (mut sum, mut total) = (0.0, 0.0);
    for f in factors {
        let w = f64::from(*weights.get(&f.key).unwrap_or(&0));
        if let (Some(s), true) = (f.score, w > 0.0) {
            sum += s * w;
            total += w;
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    (total > 0.0).then(|| (sum / total).round() as i16)
}

/// The weak factors' details, worst first.
pub fn reasons(factors: &[Factor], weights: &Weights) -> Vec<String> {
    let mut weak: Vec<&Factor> = factors
        .iter()
        .filter(|f| {
            f.score.is_some_and(|s| s < WEAK) && weights.get(&f.key).is_some_and(|w| *w > 0)
        })
        .collect();
    weak.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    weak.into_iter()
        .map(|f| format!("{}: {}", f.label, f.detail))
        .collect()
}

/// A company whose messages turned negative.
#[derive(Debug, Clone, Serialize)]
pub struct SentimentFalling {
    pub org_id: Uuid,
    pub name: String,
    /// Average, -2 to 2, in the last 30 days and the 60 before.
    pub recent: f64,
    pub previous: f64,
    /// The most negative recent messages: "T-123: reason".
    pub explanations: Vec<String>,
}

/// What a computation found.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Computed {
    pub companies: usize,
    pub declining: Vec<Declining>,
    pub sentiment_falling: Vec<SentimentFalling>,
}

/// A company that's declining.
#[derive(Debug, Clone, Serialize)]
pub struct Declining {
    pub org_id: Uuid,
    pub name: String,
    pub score: i16,
    pub previous: i16,
    pub reasons: Vec<String>,
}

/// Scores every client company in `org_id` and below for `today` (again
/// today replaces today's), and emits `client.health_declining` for those
/// that dropped [`DECLINE`] points or more from a week before, and
/// `client.sentiment_declining` for those whose messages turned negative
/// (at most once a week each).
pub async fn compute(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    today: Date,
) -> CxResult<Computed> {
    let companies: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT o.id, o.name FROM organizations o \
          WHERE o.path <@ (SELECT path FROM organizations WHERE id = $1) AND o.kind = 'client' AND o.status = 'active'",
    )
    .bind(org_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut declining = Vec::new();
    let mut falling = Vec::new();
    for (company, name) in &companies {
        let w = weights(&mut *conn, *company).await?;
        let fs = factors(&mut *conn, *company, today).await?;
        let s = score(&fs, &w);
        let why = reasons(&fs, &w);
        sqlx::query(
            "INSERT INTO cx_health (org_id, computed_on, score, factors, reasons) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (org_id, computed_on) DO UPDATE SET score = EXCLUDED.score, factors = EXCLUDED.factors, \
                 reasons = EXCLUDED.reasons",
        )
        .bind(company)
        .bind(today)
        .bind(s)
        .bind(sqlx::types::Json(&fs))
        .bind(sqlx::types::Json(&why))
        .execute(&mut *conn)
        .await?;
        let previous: Option<i16> = sqlx::query_scalar(
            "SELECT score FROM cx_health WHERE org_id = $1 AND computed_on <= $2 AND score IS NOT NULL \
              ORDER BY computed_on DESC LIMIT 1",
        )
        .bind(company)
        .bind(today - Duration::days(7))
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
        if let (Some(now), Some(before)) = (s, previous)
            && before - now >= DECLINE
        {
            outbox::emit(
                &mut *conn,
                DomainEvent {
                    event_type: "client.health_declining",
                    org_id: Some(*company),
                    subject_type: "organization",
                    subject_id: *company,
                    payload: json!({ "name": name, "score": now, "previous": before, "reasons": why }),
                },
            )
            .await?;
            declining.push(Declining {
                org_id: *company,
                name: name.clone(),
                score: now,
                previous: before,
                reasons: why,
            });
        }
        if let Some(f) = flag_sentiment(&mut *conn, *company, name, today).await? {
            falling.push(f);
        }
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("health.computed")
            .org(org_id)
            .details(json!({
                "companies": companies.len(), "declining": declining.len(), "sentiment_falling": falling.len(),
            })),
    )
    .await?;
    Ok(Computed {
        companies: companies.len(),
        declining,
        sentiment_falling: falling,
    })
}

/// Flags a company whose sentiment is falling, unless flagged in the last
/// week: emits `client.sentiment_declining` with the explanations.
async fn flag_sentiment(
    conn: &mut PgConnection,
    company: Uuid,
    name: &str,
    today: Date,
) -> CxResult<Option<SentimentFalling>> {
    let t = crate::sentiment::trend(&mut *conn, company, today).await?;
    let (Some(recent), Some(previous)) = (t.recent, t.before) else {
        return Ok(None);
    };
    if !t.falling() {
        return Ok(None);
    }
    let flagged: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM cx_health WHERE org_id = $1 AND sentiment_flagged AND computed_on BETWEEN $2 AND $3)",
    )
    .bind(company)
    .bind(today - Duration::days(7))
    .bind(today)
    .fetch_one(&mut *conn)
    .await?;
    if flagged {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE cx_health SET sentiment_flagged = true WHERE org_id = $1 AND computed_on = $2",
    )
    .bind(company)
    .bind(today)
    .execute(&mut *conn)
    .await?;
    let f = SentimentFalling {
        org_id: company,
        name: name.to_owned(),
        recent,
        previous,
        explanations: t.worst,
    };
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "client.sentiment_declining",
            org_id: Some(company),
            subject_type: "organization",
            subject_id: company,
            payload: json!({
                "name": name, "recent": (recent * 10.0).round() / 10.0, "previous": (previous * 10.0).round() / 10.0,
                "explanations": f.explanations,
            }),
        },
    )
    .await?;
    Ok(Some(f))
}

/// A snapshot: when, the score, the factors and the reasons.
type HealthRow = (
    Date,
    Option<i16>,
    sqlx::types::Json<Vec<Factor>>,
    sqlx::types::Json<Vec<String>>,
);

/// A company's latest health, with its scores over the last 90 days.
pub async fn latest(conn: &mut PgConnection, company: Uuid) -> CxResult<Option<Health>> {
    let row: Option<HealthRow> = sqlx::query_as(
        "SELECT computed_on, score, factors, reasons FROM cx_health WHERE org_id = $1 ORDER BY computed_on DESC LIMIT 1",
    )
    .bind(company)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((computed_on, score, factors, reasons)) = row else {
        return Ok(None);
    };
    let history: Vec<HealthPoint> = sqlx::query_as(
        "SELECT computed_on AS day, score FROM cx_health WHERE org_id = $1 AND computed_on >= $2 ORDER BY computed_on",
    )
    .bind(company)
    .bind(computed_on - Duration::days(90))
    .fetch_all(conn)
    .await?;
    Ok(Some(Health {
        org_id: company,
        score,
        factors: factors.0,
        reasons: reasons.0,
        computed_on,
        history,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_weigh_factors_with_data() {
        let fs = vec![
            factor("csat", Some(90.0), "a".into()),
            factor("sla", Some(30.0), "b".into()),
            factor("nps", None, "c".into()),
        ];
        let w = default_weights();
        // (90×3 + 30×3) / 6 = 60
        assert_eq!(score(&fs, &w), Some(60));
        assert_eq!(reasons(&fs, &w), ["SLA targets met: b"]);
        let mut only_csat = w.clone();
        only_csat.insert("sla".into(), 0);
        assert_eq!(score(&fs, &only_csat), Some(90));
        assert_eq!(score(&[factor("nps", None, String::new())], &w), None);
    }
}
