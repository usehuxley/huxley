//! What work requires (skills at a level, certifications) on queues and
//! ticket types, who is qualified for it, and the certification expiry
//! sweep.

use std::collections::BTreeMap;

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::people::EXPIRING_DAYS;
use crate::{WorkforceError, WorkforceResult};

pub const TARGETS: &[&str] = &["queue", "ticket_type", "ticket"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requirement {
    /// A skill, at `minLevel` or above (1 learning … 4 expert).
    #[serde(default)]
    pub skill_id: Option<Uuid>,
    #[serde(default)]
    pub min_level: Option<i16>,
    /// A certification, held and unexpired: a hard requirement.
    #[serde(default)]
    pub certification_id: Option<Uuid>,
    /// The skill's or certification's name (ignored when setting).
    #[serde(default)]
    #[sqlx(default)]
    pub name: Option<String>,
}

fn check_target(target_type: &str) -> WorkforceResult<()> {
    if TARGETS.contains(&target_type) {
        Ok(())
    } else {
        Err(WorkforceError::invalid(
            "targetType",
            "queue, ticket_type or ticket",
        ))
    }
}

/// What a queue or ticket type requires.
pub async fn of(
    conn: &mut PgConnection,
    target_type: &str,
    target_id: Uuid,
) -> WorkforceResult<Vec<Requirement>> {
    check_target(target_type)?;
    Ok(sqlx::query_as(
        "SELECT r.skill_id, r.min_level, r.certification_id, \
                coalesce(s.name, c.vendor || ' ' || c.name) AS name \
           FROM wf_requirements r \
           LEFT JOIN wf_skills s ON s.id = r.skill_id \
           LEFT JOIN wf_certifications c ON c.id = r.certification_id \
          WHERE r.target_type = $1 AND r.target_id = $2 \
          ORDER BY r.certification_id IS NULL, name",
    )
    .bind(target_type)
    .bind(target_id)
    .fetch_all(conn)
    .await?)
}

/// Replaces what a queue or ticket type requires.
pub async fn set(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    target_type: &str,
    target_id: Uuid,
    list: &[Requirement],
) -> WorkforceResult<Vec<Requirement>> {
    check_target(target_type)?;
    for r in list {
        match (r.skill_id, r.min_level, r.certification_id) {
            (Some(_), Some(level), None) if (1..=4).contains(&level) => {}
            (Some(_), _, None) => {
                return Err(WorkforceError::invalid(
                    "minLevel",
                    "a skill needs a level from 1 to 4",
                ));
            }
            (None, None, Some(_)) => {}
            _ => {
                return Err(WorkforceError::invalid(
                    "requirements",
                    "each is a skill or a certification",
                ));
            }
        }
    }
    sqlx::query("DELETE FROM wf_requirements WHERE target_type = $1 AND target_id = $2")
        .bind(target_type)
        .bind(target_id)
        .execute(&mut *conn)
        .await?;
    for r in list {
        sqlx::query(
            "INSERT INTO wf_requirements (id, target_type, target_id, skill_id, min_level, certification_id) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(huxley_core::new_id())
        .bind(target_type)
        .bind(target_id)
        .bind(r.skill_id)
        .bind(r.min_level)
        .bind(r.certification_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
                WorkforceError::invalid("requirements", "there's no such skill or certification")
            }
            _ => WorkforceError::Sqlx(e),
        })?;
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("requirements.set")
            .target(target_type, target_id)
            .details(json!({ "requirements": list })),
    )
    .await?;
    of(conn, target_type, target_id).await
}

/// How well someone fits a piece of work.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    pub user_id: Uuid,
    pub name: String,
    /// Holds every required certification (unexpired).
    pub qualified: bool,
    /// How well their skills meet the required levels, 0 to 1 (1 with no
    /// skill requirements).
    pub skill_match: f64,
    /// Requirements they don't meet, by name.
    pub missing: Vec<String>,
    /// Available at the moment asked about: not away, and within their
    /// working hours (if they have a schedule).
    pub available: bool,
    /// Why not: `away` or `off_hours`.
    pub unavailable: Option<String>,
}

/// The work `fit` is about: its queue, ticket type and ticket, each of
/// which may carry requirements.
#[derive(Debug, Clone, Copy, Default)]
pub struct Work {
    pub queue: Option<Uuid>,
    pub ticket_type: Option<Uuid>,
    pub ticket: Option<Uuid>,
}

/// How each of `people` (by default every active person) fits `work` at
/// `at` (Unix seconds; none: availability isn't considered): certifications
/// are hard requirements, then availability, then how well skill levels are
/// met (partly met counts in part). Best fits first.
pub async fn fit(
    conn: &mut PgConnection,
    work: Work,
    people: Option<&[Uuid]>,
    at: Option<i64>,
) -> WorkforceResult<Vec<Fit>> {
    let needs = needs(conn, work).await?;
    let candidates: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, display_name FROM users WHERE status = 'active' AND kind = 'human' \
            AND ($1::uuid[] IS NULL OR id = ANY($1)) ORDER BY lower(display_name)",
    )
    .bind(people.map(<[Uuid]>::to_vec))
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = candidates.iter().map(|(id, _)| *id).collect();
    let skills: Vec<(Uuid, Uuid, i16)> = sqlx::query_as(
        "SELECT user_id, skill_id, level FROM wf_user_skills WHERE user_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let certs: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT DISTINCT user_id, certification_id FROM wf_user_certifications \
          WHERE user_id = ANY($1) AND (expires_on IS NULL OR expires_on >= current_date)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut level: BTreeMap<(Uuid, Uuid), i16> = BTreeMap::new();
    for (u, s, l) in skills {
        level.insert((u, s), l);
    }
    let held: std::collections::BTreeSet<(Uuid, Uuid)> = certs.into_iter().collect();
    let unavailable = match at {
        Some(at) => crate::availability::unavailable(conn, &ids, at).await?,
        None => BTreeMap::new(),
    };
    let skill_needs: Vec<&Requirement> = needs.iter().filter(|r| r.skill_id.is_some()).collect();
    let mut fits: Vec<Fit> = candidates
        .into_iter()
        .map(|(user_id, name)| {
            let mut missing = Vec::new();
            let mut qualified = true;
            for r in needs.iter().filter(|r| r.certification_id.is_some()) {
                if !held.contains(&(user_id, r.certification_id.unwrap_or_default())) {
                    qualified = false;
                    missing.push(r.name.clone().unwrap_or_default());
                }
            }
            let mut total = 0.0;
            for r in &skill_needs {
                let want = f64::from(r.min_level.unwrap_or(1));
                let have = f64::from(
                    *level
                        .get(&(user_id, r.skill_id.unwrap_or_default()))
                        .unwrap_or(&0),
                );
                if have < want {
                    missing.push(r.name.clone().unwrap_or_default());
                }
                total += (have / want).min(1.0);
            }
            #[allow(clippy::cast_precision_loss)] // a handful of requirements
            let skill_match = if skill_needs.is_empty() {
                1.0
            } else {
                total / skill_needs.len() as f64
            };
            let why = unavailable.get(&user_id).map(|w| (*w).to_owned());
            Fit {
                user_id,
                name,
                qualified,
                skill_match,
                missing,
                available: why.is_none(),
                unavailable: why,
            }
        })
        .collect();
    fits.sort_by(|a, b| {
        b.qualified
            .cmp(&a.qualified)
            .then(b.available.cmp(&a.available))
            .then(b.skill_match.total_cmp(&a.skill_match))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(fits)
}

/// Everything `work` requires, from its queue, type and ticket; a skill
/// asked for twice counts at the higher level.
pub async fn needs(conn: &mut PgConnection, work: Work) -> WorkforceResult<Vec<Requirement>> {
    let mut all: Vec<Requirement> = Vec::new();
    for (target, id) in [
        ("queue", work.queue),
        ("ticket_type", work.ticket_type),
        ("ticket", work.ticket),
    ] {
        let Some(id) = id else { continue };
        for r in of(conn, target, id).await? {
            match all.iter_mut().find(|a| {
                (a.skill_id.is_some() && a.skill_id == r.skill_id)
                    || (a.certification_id.is_some() && a.certification_id == r.certification_id)
            }) {
                Some(a) => a.min_level = a.min_level.max(r.min_level),
                None => all.push(r),
            }
        }
    }
    Ok(all)
}

/// Announces certifications expiring within 30 days and those expired,
/// once each: `certification.expiring` and `certification.expired`.
/// System scope; returns how many it announced.
pub async fn sweep(conn: &mut PgConnection) -> WorkforceResult<usize> {
    let mut announced = 0;
    for (event, column, condition) in [
        (
            "certification.expiring",
            "warned_at",
            "u.expires_on >= current_date AND u.expires_on <= current_date + $1",
        ),
        (
            "certification.expired",
            "expired_at",
            "u.expires_on < current_date AND $1 IS NOT NULL",
        ),
    ] {
        let hits: Vec<(Uuid, Uuid, String, String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE wf_user_certifications u SET {column} = now() \
               FROM wf_certifications c, users p \
              WHERE c.id = u.certification_id AND p.id = u.user_id AND u.{column} IS NULL AND {condition} \
              RETURNING u.id, u.user_id, p.display_name, c.vendor, c.name, to_char(u.expires_on, 'YYYY-MM-DD')"
        )))
        .bind(EXPIRING_DAYS)
        .fetch_all(&mut *conn)
        .await?;
        for (id, user_id, person, vendor, name, expires_on) in hits {
            outbox::emit(
                conn,
                DomainEvent {
                    event_type: event,
                    org_id: None,
                    subject_type: "user",
                    subject_id: user_id,
                    payload: json!({
                        "user_id": user_id,
                        "person": person,
                        "held_id": id,
                        "vendor": vendor,
                        "certification": name,
                        "expires_on": expires_on,
                    }),
                },
            )
            .await?;
            announced += 1;
        }
    }
    Ok(announced)
}
