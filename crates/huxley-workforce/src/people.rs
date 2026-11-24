//! People: their skills (self-assessed or verified) and certifications.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkforceError, WorkforceResult, text};

/// Days before expiry that count as "expiring soon".
pub const EXPIRING_DAYS: i32 = 30;

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PersonSummary {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub skills: i64,
    /// Certifications held, unexpired.
    pub certifications: i64,
    /// Certifications expiring within 30 days.
    pub expiring: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PersonSkill {
    pub skill_id: Uuid,
    pub name: String,
    pub category: String,
    /// 1 learning, 2 working, 3 proficient, 4 expert.
    pub level: i16,
    /// Who verified the level (none: self-assessed).
    pub verified_by: Option<String>,
    /// `YYYY-MM-DD`.
    pub verified_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PersonCertification {
    pub id: Uuid,
    pub certification_id: Uuid,
    pub vendor: String,
    pub name: String,
    pub code: String,
    /// `YYYY-MM-DD`.
    pub earned_on: String,
    /// `YYYY-MM-DD`; none: it doesn't expire.
    pub expires_on: Option<String>,
    pub credential_id: String,
    pub verify_url: Option<String>,
    /// `valid`, `expiring` (within 30 days) or `expired`.
    pub state: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub skills: Vec<PersonSkill>,
    pub certifications: Vec<PersonCertification>,
}

/// Active people (not service accounts), by name.
pub async fn people(conn: &mut PgConnection) -> WorkforceResult<Vec<PersonSummary>> {
    Ok(sqlx::query_as(
        "SELECT u.id AS user_id, u.display_name AS name, u.email, \
                (SELECT count(*) FROM wf_user_skills s JOIN wf_skills k ON k.id = s.skill_id \
                  WHERE s.user_id = u.id AND k.archived_at IS NULL) AS skills, \
                (SELECT count(*) FROM wf_user_certifications c WHERE c.user_id = u.id \
                  AND (c.expires_on IS NULL OR c.expires_on >= current_date)) AS certifications, \
                (SELECT count(*) FROM wf_user_certifications c WHERE c.user_id = u.id \
                  AND c.expires_on BETWEEN current_date AND current_date + $1) AS expiring \
           FROM users u WHERE u.status = 'active' AND u.kind = 'human' \
          ORDER BY lower(u.display_name)",
    )
    .bind(EXPIRING_DAYS)
    .fetch_all(conn)
    .await?)
}

pub async fn person(conn: &mut PgConnection, user_id: Uuid) -> WorkforceResult<Person> {
    let (name, email): (String, String) =
        sqlx::query_as("SELECT display_name, email FROM users WHERE id = $1 AND kind = 'human'")
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(WorkforceError::NotFound)?;
    let skills = sqlx::query_as(
        "SELECT s.skill_id, k.name, k.category, s.level, v.display_name AS verified_by, \
                to_char(s.verified_at, 'YYYY-MM-DD') AS verified_on \
           FROM wf_user_skills s JOIN wf_skills k ON k.id = s.skill_id \
           LEFT JOIN users v ON v.id = s.verified_by \
          WHERE s.user_id = $1 AND k.archived_at IS NULL \
          ORDER BY lower(k.category), lower(k.name)",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await?;
    let certifications = sqlx::query_as(
        "SELECT u.id, u.certification_id, c.vendor, c.name, c.code, \
                to_char(u.earned_on, 'YYYY-MM-DD') AS earned_on, \
                to_char(u.expires_on, 'YYYY-MM-DD') AS expires_on, u.credential_id, u.verify_url, \
                CASE WHEN u.expires_on < current_date THEN 'expired' \
                     WHEN u.expires_on <= current_date + $2 THEN 'expiring' ELSE 'valid' END AS state \
           FROM wf_user_certifications u JOIN wf_certifications c ON c.id = u.certification_id \
          WHERE u.user_id = $1 ORDER BY u.expires_on NULLS LAST, lower(c.name)",
    )
    .bind(user_id)
    .bind(EXPIRING_DAYS)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Person {
        user_id,
        name,
        email,
        skills,
        certifications,
    })
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLevel {
    /// 1 learning … 4 expert; none removes the skill.
    pub level: Option<i16>,
    /// Mark the level verified (needs `workforce:manage`, and not your own).
    #[serde(default)]
    pub verified: bool,
}

/// Sets (or removes) one of a person's skills. `verifier` is who verifies
/// it, if it's verified; anyone else's change makes it self-assessed again.
pub async fn set_skill(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    skill_id: Uuid,
    input: &SkillLevel,
    verifier: Option<Uuid>,
) -> WorkforceResult<Person> {
    let skill_name: String =
        sqlx::query_scalar("SELECT name FROM wf_skills WHERE id = $1 AND archived_at IS NULL")
            .bind(skill_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| WorkforceError::invalid("skillId", "there's no such skill"))?;
    match input.level {
        None => {
            sqlx::query("DELETE FROM wf_user_skills WHERE user_id = $1 AND skill_id = $2")
                .bind(user_id)
                .bind(skill_id)
                .execute(&mut *conn)
                .await?;
        }
        Some(level) if (1..=4).contains(&level) => {
            sqlx::query(
                "INSERT INTO wf_user_skills (user_id, skill_id, level, verified_by, verified_at) \
                 VALUES ($1, $2, $3, $4, CASE WHEN $4::uuid IS NULL THEN NULL ELSE now() END) \
                 ON CONFLICT (user_id, skill_id) DO UPDATE SET level = EXCLUDED.level, \
                   verified_by = EXCLUDED.verified_by, verified_at = EXCLUDED.verified_at, updated_at = now()",
            )
            .bind(user_id)
            .bind(skill_id)
            .bind(level)
            .bind(verifier)
            .execute(&mut *conn)
            .await?;
        }
        Some(_) => {
            return Err(WorkforceError::invalid(
                "level",
                "1 (learning) to 4 (expert)",
            ));
        }
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("person.skill_set")
            .target("user", user_id)
            .details(json!({ "skill": skill_name, "level": input.level, "verified": verifier.is_some() })),
    )
    .await?;
    person(conn, user_id).await
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationHeld {
    /// The certification (when adding one; ignored when changing it).
    #[serde(default)]
    pub certification_id: Option<Uuid>,
    /// `YYYY-MM-DD`.
    pub earned_on: String,
    /// `YYYY-MM-DD`; left out, from the certification's validity.
    #[serde(default)]
    pub expires_on: Option<String>,
    #[serde(default)]
    pub credential_id: Option<String>,
    /// An `https://` link where the credential can be checked.
    #[serde(default)]
    pub verify_url: Option<String>,
}

fn date(field: &str, value: &str) -> WorkforceResult<String> {
    let v = value.trim();
    let ok = v.len() == 10
        && v.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if ok {
        Ok(v.to_owned())
    } else {
        Err(WorkforceError::invalid(field, "a date, as YYYY-MM-DD"))
    }
}

fn checked(
    input: &CertificationHeld,
) -> WorkforceResult<(String, Option<String>, String, Option<String>)> {
    let earned = date("earnedOn", &input.earned_on)?;
    let expires = input
        .expires_on
        .as_deref()
        .map(|e| date("expiresOn", e))
        .transpose()?;
    let credential = text("credentialId", input.credential_id.as_deref(), 200)?.unwrap_or_default();
    let url = text("verifyUrl", input.verify_url.as_deref(), 2000)?;
    if url.as_deref().is_some_and(|u| !u.starts_with("https://")) {
        return Err(WorkforceError::invalid("verifyUrl", "an https:// link"));
    }
    Ok((earned, expires, credential, url))
}

/// Postgres refusing a date or the expiry order, as a field error.
fn date_error(e: sqlx::Error) -> WorkforceError {
    match &e {
        sqlx::Error::Database(d) if matches!(d.code().as_deref(), Some("22007" | "22008")) => {
            WorkforceError::invalid("earnedOn", "not a real date")
        }
        sqlx::Error::Database(d) if d.is_check_violation() => {
            WorkforceError::invalid("expiresOn", "can't be before it was earned")
        }
        sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
            WorkforceError::invalid("certificationId", "there's no such certification")
        }
        _ => WorkforceError::Sqlx(e),
    }
}

/// Adds a certification someone holds. Without `expiresOn`, it expires
/// after the certification's validity (if it has one).
pub async fn add_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    input: &CertificationHeld,
) -> WorkforceResult<Person> {
    let (earned, expires, credential, url) = checked(input)?;
    let certification = input
        .certification_id
        .ok_or_else(|| WorkforceError::invalid("certificationId", "which certification"))?;
    let id = huxley_core::new_id();
    let inserted = sqlx::query(
        "INSERT INTO wf_user_certifications (id, user_id, certification_id, earned_on, expires_on, \
                                             credential_id, verify_url) \
         SELECT $1, $2, c.id, $4::date, \
                coalesce($5::date, ($4::date + make_interval(months => c.validity_months))::date), $6, $7 \
           FROM wf_certifications c WHERE c.id = $3 AND c.archived_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .bind(certification)
    .bind(&earned)
    .bind(&expires)
    .bind(&credential)
    .bind(&url)
    .execute(&mut *conn)
    .await
    .map_err(date_error)?
    .rows_affected();
    if inserted == 0 {
        return Err(WorkforceError::invalid(
            "certificationId",
            "there's no such certification",
        ));
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("person.certification_added")
            .target("user", user_id)
            .details(json!({ "certification": input.certification_id, "earnedOn": earned })),
    )
    .await?;
    person(conn, user_id).await
}

/// Changes a held certification (e.g. renewed: a new expiry).
pub async fn update_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    id: Uuid,
    input: &CertificationHeld,
) -> WorkforceResult<Person> {
    let (earned, expires, credential, url) = checked(input)?;
    let rows = sqlx::query(
        "UPDATE wf_user_certifications u SET earned_on = $3::date, \
                expires_on = coalesce($4::date, ($3::date + make_interval(months => c.validity_months))::date), \
                credential_id = $5, verify_url = $6, \
                -- Renewed: announce its next expiry again.
                warned_at = NULL, expired_at = NULL \
           FROM wf_certifications c \
          WHERE u.id = $1 AND u.user_id = $2 AND c.id = u.certification_id",
    )
    .bind(id)
    .bind(user_id)
    .bind(&earned)
    .bind(&expires)
    .bind(&credential)
    .bind(&url)
    .execute(&mut *conn)
    .await
    .map_err(date_error)?
    .rows_affected();
    if rows == 0 {
        return Err(WorkforceError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("person.certification_updated")
            .target("user", user_id)
            .details(json!({ "held": id, "earnedOn": earned, "expiresOn": expires })),
    )
    .await?;
    person(conn, user_id).await
}

pub async fn remove_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user_id: Uuid,
    id: Uuid,
) -> WorkforceResult<Person> {
    let rows = sqlx::query("DELETE FROM wf_user_certifications WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(WorkforceError::NotFound);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("person.certification_removed")
            .target("user", user_id)
            .details(json!({ "held": id })),
    )
    .await?;
    person(conn, user_id).await
}
