//! Skill-gap insights (design doc §8.8, ADR-049): where few people meet the
//! work's requirements, work done without them, and certifications about to
//! expire with what they'd leave uncovered. Read with system scope.

use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::WorkforceResult;
use crate::requirements::{self, Work};

/// Recently resolved tickets looked at for unqualified work.
const RECENT_TICKETS: i64 = 200;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SkillCoverage)]
pub struct Coverage {
    /// `queue` or `ticket_type`.
    pub target_type: String,
    pub target_id: Uuid,
    pub name: String,
    /// What it requires, by name.
    pub requirements: Vec<String>,
    /// Who meets every requirement.
    pub qualified: Vec<String>,
    /// Everyone who could be asked (its members, for a queue).
    pub pool: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = UnqualifiedWork)]
pub struct Unqualified {
    /// The requirement that wasn't met.
    pub requirement: String,
    /// Resolved tickets (of the last 200, 90 days) worked without it.
    pub tickets: i64,
    pub people: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ExpiringCertification)]
pub struct Expiring {
    pub user_id: Uuid,
    pub person: String,
    pub certification: String,
    pub expires_on: String,
    /// Work nobody else is qualified for.
    pub uncovers: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SkillGapInsights)]
pub struct Insights {
    pub coverage: Vec<Coverage>,
    pub unqualified: Vec<Unqualified>,
    pub expiring: Vec<Expiring>,
    /// The findings worth acting on, most serious first, in a line each.
    pub summary: Vec<String>,
}

/// Who meets every requirement of each queue and ticket type that has some.
async fn coverage(conn: &mut PgConnection) -> WorkforceResult<Vec<Coverage>> {
    let targets: Vec<(String, Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT r.target_type, r.target_id, coalesce(q.name, t.name) FROM wf_requirements r \
           LEFT JOIN sd_queues q ON r.target_type = 'queue' AND q.id = r.target_id AND q.archived_at IS NULL \
           LEFT JOIN sd_ticket_types t ON r.target_type = 'ticket_type' AND t.id = r.target_id \
          WHERE r.target_type IN ('queue', 'ticket_type') AND coalesce(q.name, t.name) IS NOT NULL \
          ORDER BY 3",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(targets.len());
    for (target_type, target_id, name) in targets {
        let members: Vec<Uuid> = if target_type == "queue" {
            sqlx::query_scalar(
                "SELECT DISTINCT u.id FROM sd_queue_members m LEFT JOIN team_members tm ON tm.team_id = m.team_id \
                   JOIN users u ON u.id = coalesce(m.user_id, tm.user_id) \
                  WHERE m.queue_id = $1 AND u.status = 'active' AND u.kind = 'human'",
            )
            .bind(target_id)
            .fetch_all(&mut *conn)
            .await?
        } else {
            Vec::new()
        };
        let work = if target_type == "queue" {
            Work {
                queue: Some(target_id),
                ..Work::default()
            }
        } else {
            Work {
                ticket_type: Some(target_id),
                ..Work::default()
            }
        };
        let needs = requirements::needs(&mut *conn, work).await?;
        let people = (!members.is_empty()).then_some(members.as_slice());
        let fits = requirements::fit(&mut *conn, work, people, None).await?;
        out.push(Coverage {
            target_type,
            target_id,
            name,
            requirements: needs
                .into_iter()
                .filter_map(|r| r.name.map(|n| n.trim().to_owned()))
                .collect(),
            qualified: fits
                .iter()
                .filter(|f| f.qualified && f.missing.is_empty())
                .map(|f| f.name.clone())
                .collect(),
            pool: fits.len(),
        });
    }
    Ok(out)
}

/// Requirements recently resolved tickets were worked without.
async fn unqualified(conn: &mut PgConnection) -> WorkforceResult<Vec<Unqualified>> {
    let tickets: Vec<(Uuid, Option<Uuid>, Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, queue_id, type_id, assignee_user_id FROM sd_tickets \
          WHERE assignee_user_id IS NOT NULL AND resolved_at >= now() - interval '90 days' \
          ORDER BY resolved_at DESC LIMIT $1",
    )
    .bind(RECENT_TICKETS)
    .fetch_all(&mut *conn)
    .await?;
    let mut tally: std::collections::BTreeMap<String, (i64, std::collections::BTreeSet<String>)> =
        std::collections::BTreeMap::new();
    for (ticket, queue, ticket_type, assignee) in tickets {
        let work = Work {
            queue,
            ticket_type: Some(ticket_type),
            ticket: Some(ticket),
        };
        for fit in requirements::fit(&mut *conn, work, Some(&[assignee]), None).await? {
            for missing in fit.missing {
                let entry = tally.entry(missing.trim().to_owned()).or_default();
                entry.0 += 1;
                entry.1.insert(fit.name.clone());
            }
        }
    }
    let mut out: Vec<Unqualified> = tally
        .into_iter()
        .map(|(requirement, (tickets, people))| Unqualified {
            requirement,
            tickets,
            people: people.into_iter().collect(),
        })
        .collect();
    out.sort_by(|a, b| {
        b.tickets
            .cmp(&a.tickets)
            .then(a.requirement.cmp(&b.requirement))
    });
    Ok(out)
}

/// Certifications expiring within `days`, with the work only that holder
/// is qualified for.
async fn expiring(
    conn: &mut PgConnection,
    days: i32,
    coverage: &[Coverage],
) -> WorkforceResult<Vec<Expiring>> {
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT u.user_id, p.display_name, trim(c.vendor || ' ' || c.name), to_char(u.expires_on, 'YYYY-MM-DD') \
           FROM wf_user_certifications u JOIN wf_certifications c ON c.id = u.certification_id \
           JOIN users p ON p.id = u.user_id \
          WHERE p.status = 'active' AND u.expires_on >= current_date AND u.expires_on <= current_date + $1 \
          ORDER BY u.expires_on, p.display_name",
    )
    .bind(days)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, person, certification, expires_on)| {
            let uncovers = coverage
                .iter()
                .filter(|c| c.qualified.len() == 1 && c.qualified[0] == person)
                .filter(|c| c.requirements.iter().any(|r| r.trim() == certification))
                .map(|c| c.name.clone())
                .collect();
            Expiring {
                user_id,
                person,
                certification,
                expires_on,
                uncovers,
            }
        })
        .collect())
}

/// The findings worth acting on, a line each.
fn summarize(
    coverage: &[Coverage],
    unqualified: &[Unqualified],
    expiring: &[Expiring],
) -> Vec<String> {
    let mut out = Vec::new();
    for c in coverage.iter().filter(|c| c.qualified.is_empty()) {
        out.push(format!(
            "Nobody meets everything {} requires ({})",
            c.name,
            c.requirements.join(", ")
        ));
    }
    for e in expiring.iter().filter(|e| !e.uncovers.is_empty()) {
        out.push(format!(
            "{}'s {} expires {}: nobody else is qualified for {}",
            e.person,
            e.certification,
            e.expires_on,
            e.uncovers.join(", ")
        ));
    }
    for c in coverage.iter().filter(|c| c.qualified.len() == 1) {
        out.push(format!(
            "Only {} meets everything {} requires",
            c.qualified[0], c.name
        ));
    }
    for u in unqualified.iter().take(5) {
        out.push(format!(
            "{} recent ticket{} worked without {} (by {})",
            u.tickets,
            if u.tickets == 1 { " was" } else { "s were" },
            u.requirement,
            u.people.join(", ")
        ));
    }
    for e in expiring.iter().filter(|e| e.uncovers.is_empty()) {
        out.push(format!(
            "{}'s {} expires {}",
            e.person, e.certification, e.expires_on
        ));
    }
    out
}

/// Skill-gap insights now: coverage, unqualified work, and certifications
/// expiring within `days`.
pub async fn insights(conn: &mut PgConnection, days: i32) -> WorkforceResult<Insights> {
    let coverage = coverage(&mut *conn).await?;
    let unqualified = unqualified(&mut *conn).await?;
    let expiring = expiring(&mut *conn, days, &coverage).await?;
    let summary = summarize(&coverage, &unqualified, &expiring);
    Ok(Insights {
        coverage,
        unqualified,
        expiring,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_puts_uncovered_work_first() {
        let coverage = vec![
            Coverage {
                target_type: "queue".into(),
                target_id: Uuid::nil(),
                name: "Network".into(),
                requirements: vec!["CCNA".into()],
                qualified: vec!["Ada".into()],
                pool: 3,
            },
            Coverage {
                target_type: "ticket_type".into(),
                target_id: Uuid::nil(),
                name: "Change".into(),
                requirements: vec!["ITIL".into()],
                qualified: vec![],
                pool: 3,
            },
        ];
        let unqualified = vec![Unqualified {
            requirement: "CCNA".into(),
            tickets: 4,
            people: vec!["Bob".into()],
        }];
        let expiring = vec![Expiring {
            user_id: Uuid::nil(),
            person: "Ada".into(),
            certification: "Cisco CCNA".into(),
            expires_on: "2026-12-20".into(),
            uncovers: vec!["Network".into()],
        }];
        assert_eq!(
            summarize(&coverage, &unqualified, &expiring),
            [
                "Nobody meets everything Change requires (ITIL)",
                "Ada's Cisco CCNA expires 2026-12-20: nobody else is qualified for Network",
                "Only Ada meets everything Network requires",
                "4 recent tickets were worked without CCNA (by Bob)",
            ]
        );
    }
}
