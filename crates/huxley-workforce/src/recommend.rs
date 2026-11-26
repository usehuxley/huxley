//! Dispatch recommendations (design doc §8.8, ADR-048): who should take a
//! ticket, best first, each with the reasons and cautions behind the
//! ranking. Skills and certifications (the queue's, the type's and the
//! ticket's own requirements), availability, open load, history in the
//! queue, and familiarity with the client.

use std::collections::BTreeMap;

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::requirements::{self, Work};
use crate::{WorkforceError, WorkforceResult};

/// The most points each factor gives (they add up to 100).
const SKILLS: f64 = 40.0;
const AVAILABLE: f64 = 20.0;
const LOAD: f64 = 15.0;
const HISTORY: f64 = 15.0;
const FAMILIAR: f64 = 10.0;
/// Resolved tickets in the queue (90 days) for full history points.
const HISTORY_FULL: i64 = 10;
/// The client's tickets (180 days) for full familiarity points.
const FAMILIAR_FULL: i64 = 5;
/// The most an unqualified person scores.
const UNQUALIFIED_CAP: f64 = 30.0;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub user_id: Uuid,
    pub name: String,
    /// 0 to 100.
    pub score: i16,
    /// Holds every required certification.
    pub qualified: bool,
    pub available: bool,
    pub open_tickets: i64,
    /// Tickets in this queue they resolved in the last 90 days.
    pub queue_resolved: i64,
    /// The client's tickets they worked in the last 180 days.
    pub client_tickets: i64,
    /// Assigned the ticket now.
    pub assigned: bool,
    /// Why they rank where they do.
    pub reasons: Vec<String>,
    /// What counts against them.
    pub cautions: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct TicketRow {
    org_id: Uuid,
    org_name: String,
    queue_id: Option<Uuid>,
    queue_name: Option<String>,
    type_id: Uuid,
    assignee_user_id: Option<Uuid>,
}

/// The people who could take it: the queue's members (directly or by
/// team), or without any, those who may work tickets there.
async fn candidates(conn: &mut PgConnection, t: &TicketRow) -> WorkforceResult<Vec<Uuid>> {
    let members: Vec<Uuid> = match t.queue_id {
        Some(q) => {
            sqlx::query_scalar(
                "SELECT DISTINCT u.id FROM sd_queue_members m \
               LEFT JOIN team_members tm ON tm.team_id = m.team_id \
               JOIN users u ON u.id = coalesce(m.user_id, tm.user_id) \
              WHERE m.queue_id = $1 AND u.status = 'active' AND u.kind = 'human'",
            )
            .bind(q)
            .fetch_all(&mut *conn)
            .await?
        }
        None => Vec::new(),
    };
    if !members.is_empty() {
        return Ok(members);
    }
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM role_assignments a JOIN roles r ON r.id = a.role_id \
           JOIN organizations o ON o.id = a.org_id JOIN users u ON u.id = a.user_id \
          WHERE o.path @> (SELECT path FROM organizations WHERE id = $1) \
            AND (r.permissions @> '{ticket:write}' OR r.permissions @> '{*}') \
            AND u.status = 'active' AND u.kind = 'human'",
    )
    .bind(t.org_id)
    .fetch_all(conn)
    .await?)
}

type Tally = BTreeMap<Uuid, i64>;

/// Per person: open tickets, tickets in `queue` resolved in 90 days, and
/// `client`'s tickets in 180 days.
async fn tallies(
    conn: &mut PgConnection,
    people: &[Uuid],
    queue: Option<Uuid>,
    client: Uuid,
) -> WorkforceResult<(Tally, Tally, Tally)> {
    let open: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT assignee_user_id, count(*) FROM sd_tickets \
          WHERE assignee_user_id = ANY($1) AND resolved_at IS NULL AND closed_at IS NULL GROUP BY assignee_user_id",
    )
    .bind(people)
    .fetch_all(&mut *conn)
    .await?;
    let history: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT assignee_user_id, count(*) FROM sd_tickets \
          WHERE assignee_user_id = ANY($1) AND queue_id = $2 AND resolved_at >= now() - interval '90 days' \
          GROUP BY assignee_user_id",
    )
    .bind(people)
    .bind(queue)
    .fetch_all(&mut *conn)
    .await?;
    let familiar: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT assignee_user_id, count(*) FROM sd_tickets \
          WHERE assignee_user_id = ANY($1) AND org_id = $2 AND created_at >= now() - interval '180 days' \
          GROUP BY assignee_user_id",
    )
    .bind(people)
    .bind(client)
    .fetch_all(conn)
    .await?;
    Ok((
        open.into_iter().collect(),
        history.into_iter().collect(),
        familiar.into_iter().collect(),
    ))
}

/// Who should take `ticket_id`, best first (at most `limit`), as of `at`
/// (Unix seconds). With `workforce` off, skills and availability aren't
/// weighed.
pub async fn for_ticket(
    conn: &mut PgConnection,
    ticket_id: Uuid,
    at: i64,
    workforce: bool,
    limit: usize,
) -> WorkforceResult<Vec<Recommendation>> {
    let t: TicketRow = sqlx::query_as(
        "SELECT t.org_id, o.name AS org_name, t.queue_id, q.name AS queue_name, t.type_id, t.assignee_user_id \
           FROM sd_tickets t JOIN organizations o ON o.id = t.org_id LEFT JOIN sd_queues q ON q.id = t.queue_id \
          WHERE t.id = $1",
    )
    .bind(ticket_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(WorkforceError::NotFound)?;
    let people = candidates(&mut *conn, &t).await?;
    if people.is_empty() {
        return Ok(Vec::new());
    }
    let names: BTreeMap<Uuid, String> =
        sqlx::query_as("SELECT id, display_name FROM users WHERE id = ANY($1)")
            .bind(&people)
            .fetch_all(&mut *conn)
            .await
            .map(|rows: Vec<(Uuid, String)>| rows.into_iter().collect())?;
    let work = Work {
        queue: t.queue_id,
        ticket_type: Some(t.type_id),
        ticket: Some(ticket_id),
    };
    let has_skill_needs = workforce
        && requirements::needs(&mut *conn, work)
            .await?
            .iter()
            .any(|r| r.skill_id.is_some());
    let fits: BTreeMap<Uuid, requirements::Fit> = if workforce {
        requirements::fit(&mut *conn, work, Some(&people), Some(at))
            .await?
            .into_iter()
            .map(|f| (f.user_id, f))
            .collect()
    } else {
        BTreeMap::new()
    };
    let (open, history, familiar) = tallies(&mut *conn, &people, t.queue_id, t.org_id).await?;
    let lightest = people
        .iter()
        .map(|p| open.get(p).copied().unwrap_or(0))
        .min()
        .unwrap_or(0);
    let mut out: Vec<Recommendation> = people
        .iter()
        .map(|&user_id| {
            let fit = fits.get(&user_id);
            let open_tickets = open.get(&user_id).copied().unwrap_or(0);
            let queue_resolved = history.get(&user_id).copied().unwrap_or(0);
            let client_tickets = familiar.get(&user_id).copied().unwrap_or(0);
            explain(
                Facts {
                    user_id,
                    name: names.get(&user_id).cloned().unwrap_or_default(),
                    fit,
                    has_skill_needs,
                    open_tickets,
                    lightest,
                    queue_resolved,
                    client_tickets,
                    assigned: t.assignee_user_id == Some(user_id),
                },
                t.queue_name.as_deref(),
                &t.org_name,
            )
        })
        .collect();
    out.sort_by(|a, b| {
        b.qualified
            .cmp(&a.qualified)
            .then(b.score.cmp(&a.score))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out.truncate(limit);
    Ok(out)
}

/// What's known about one person.
struct Facts<'a> {
    user_id: Uuid,
    name: String,
    fit: Option<&'a requirements::Fit>,
    has_skill_needs: bool,
    open_tickets: i64,
    lightest: i64,
    queue_resolved: i64,
    client_tickets: i64,
    assigned: bool,
}

/// Scores one person and says why.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn explain(f: Facts<'_>, queue: Option<&str>, client: &str) -> Recommendation {
    let (mut reasons, mut cautions) = (Vec::new(), Vec::new());
    let qualified = f.fit.is_none_or(|x| x.qualified);
    let available = f.fit.is_none_or(|x| x.available);
    let skill_match = f.fit.map_or(1.0, |x| x.skill_match);
    let mut score = SKILLS * skill_match;
    if f.has_skill_needs {
        if skill_match >= 1.0 {
            reasons.push("Meets every skill level it needs".to_owned());
        } else if skill_match > 0.0 {
            reasons.push(format!(
                "Meets {:.0}% of the skill levels it needs",
                skill_match * 100.0
            ));
        }
    }
    if let Some(fit) = f.fit.filter(|x| !x.missing.is_empty()) {
        cautions.push(format!("Missing: {}", fit.missing.join(", ")));
    }
    if available {
        score += AVAILABLE;
        if f.fit.is_some() {
            reasons.push("Available now".to_owned());
        }
    } else {
        cautions.push(match f.fit.and_then(|x| x.unavailable.as_deref()) {
            Some("off_hours") => "Outside their working hours".to_owned(),
            _ => "Away".to_owned(),
        });
    }
    score += LOAD * (f.lightest + 1) as f64 / (f.open_tickets + 1) as f64;
    reasons.push(format!(
        "{} open ticket{}{}",
        f.open_tickets,
        if f.open_tickets == 1 { "" } else { "s" },
        if f.open_tickets == f.lightest {
            " (the lightest load)"
        } else {
            ""
        }
    ));
    score += HISTORY * f.queue_resolved.min(HISTORY_FULL) as f64 / HISTORY_FULL as f64;
    if f.queue_resolved > 0 {
        reasons.push(format!(
            "Resolved {} in {} lately",
            f.queue_resolved,
            queue.unwrap_or("this queue")
        ));
    }
    score += FAMILIAR * f.client_tickets.min(FAMILIAR_FULL) as f64 / FAMILIAR_FULL as f64;
    if f.client_tickets > 0 {
        reasons.push(format!("Worked {} of {client}'s tickets", f.client_tickets));
    }
    if !qualified {
        score = score.min(UNQUALIFIED_CAP);
    }
    if f.assigned {
        cautions.push("Assigned now".to_owned());
    }
    Recommendation {
        user_id: f.user_id,
        name: f.name,
        score: score.round() as i16,
        qualified,
        available,
        open_tickets: f.open_tickets,
        queue_resolved: f.queue_resolved,
        client_tickets: f.client_tickets,
        assigned: f.assigned,
        reasons,
        cautions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(open: i64, history: i64, familiar: i64) -> Facts<'static> {
        Facts {
            user_id: Uuid::nil(),
            name: "Ada".into(),
            fit: None,
            has_skill_needs: false,
            open_tickets: open,
            lightest: 1,
            queue_resolved: history,
            client_tickets: familiar,
            assigned: false,
        }
    }

    #[test]
    fn scores_add_up_with_reasons() {
        // No workforce data: skills and availability count in full.
        let r = explain(facts(1, 10, 5), Some("Network"), "Acme");
        assert_eq!(r.score, 100);
        assert_eq!(
            r.reasons,
            [
                "1 open ticket (the lightest load)",
                "Resolved 10 in Network lately",
                "Worked 5 of Acme's tickets"
            ]
        );
        // More load, less history and familiarity.
        let r = explain(facts(3, 5, 0), None, "Acme");
        // 40 + 20 + 15 × 2/4 + 15 × 0.5 = 75
        assert_eq!(r.score, 75);
        assert_eq!(
            r.reasons,
            ["3 open tickets", "Resolved 5 in this queue lately"]
        );
    }

    #[test]
    fn unqualified_and_away_people_are_cautioned() {
        let fit = requirements::Fit {
            user_id: Uuid::nil(),
            name: "Ada".into(),
            qualified: false,
            skill_match: 0.5,
            missing: vec!["CCNA".into()],
            available: false,
            unavailable: Some("off_hours".into()),
        };
        let mut f = facts(1, 10, 5);
        f.fit = Some(&fit);
        f.has_skill_needs = true;
        let r = explain(f, None, "Acme");
        assert_eq!(r.score, 30);
        assert_eq!(r.cautions, ["Missing: CCNA", "Outside their working hours"]);
        assert_eq!(r.reasons[0], "Meets 50% of the skill levels it needs");
    }
}
