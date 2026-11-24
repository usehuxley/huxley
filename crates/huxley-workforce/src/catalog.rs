//! The skills and certifications catalogs.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{WorkforceError, WorkforceResult, taken, text};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: Uuid,
    pub name: String,
    /// e.g. `Microsoft 365`, `Networking`.
    pub category: String,
    pub description: String,
    /// People with this skill.
    pub people: i64,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillInput {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SKILLS: &str = "SELECT s.id, s.name, s.category, s.description, s.version, \
    (SELECT count(*) FROM wf_user_skills u WHERE u.skill_id = s.id) AS people FROM wf_skills s";

/// Active skills by category, then name.
pub async fn skills(conn: &mut PgConnection) -> WorkforceResult<Vec<Skill>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SKILLS} WHERE s.archived_at IS NULL ORDER BY lower(s.category), lower(s.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn skill(conn: &mut PgConnection, id: Uuid) -> WorkforceResult<Skill> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SKILLS} WHERE s.id = $1 AND s.archived_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(WorkforceError::NotFound)
}

fn skill_fields(input: &SkillInput) -> WorkforceResult<(String, String, String)> {
    Ok((
        text("name", Some(&input.name), 100)?
            .ok_or_else(|| WorkforceError::invalid("name", "a skill needs a name"))?,
        text("category", input.category.as_deref(), 100)?.unwrap_or_default(),
        text("description", input.description.as_deref(), 2000)?.unwrap_or_default(),
    ))
}

pub async fn create_skill(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &SkillInput,
) -> WorkforceResult<Skill> {
    let (name, category, description) = skill_fields(input)?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO wf_skills (id, name, category, description) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(&name)
        .bind(&category)
        .bind(&description)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            taken(
                e,
                "wf_skills_name",
                "name",
                "there's already a skill with this name",
            )
        })?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("skill.created")
            .target("skill", id)
            .details(json!({ "name": name, "category": category })),
    )
    .await?;
    skill(conn, id).await
}

pub async fn update_skill(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &SkillInput,
) -> WorkforceResult<Skill> {
    let (name, category, description) = skill_fields(input)?;
    let rows = sqlx::query(
        "UPDATE wf_skills SET name = $3, category = $4, description = $5, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&name)
    .bind(&category)
    .bind(&description)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        taken(
            e,
            "wf_skills_name",
            "name",
            "there's already a skill with this name",
        )
    })?
    .rows_affected();
    if rows == 0 {
        skill(conn, id).await?;
        return Err(WorkforceError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("skill.updated")
            .target("skill", id)
            .details(json!({ "name": name, "category": category })),
    )
    .await?;
    skill(conn, id).await
}

/// Archives a skill: people's levels and requirements on it go too.
pub async fn archive_skill(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> WorkforceResult<()> {
    let s = skill(conn, id).await?;
    sqlx::query("UPDATE wf_skills SET archived_at = now(), version = version + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM wf_requirements WHERE skill_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM wf_user_skills WHERE skill_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("skill.archived")
            .target("skill", id)
            .details(json!({ "name": s.name })),
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Certification {
    pub id: Uuid,
    /// e.g. `Microsoft`, `Cisco`.
    pub vendor: String,
    pub name: String,
    /// The vendor's code, e.g. `AZ-104`.
    pub code: String,
    /// How long it's valid once earned; none: it doesn't expire.
    pub validity_months: Option<i32>,
    pub description: String,
    /// People holding it, unexpired.
    pub holders: i64,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CertificationInput {
    pub vendor: String,
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub validity_months: Option<i32>,
    #[serde(default)]
    pub description: Option<String>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const CERTS: &str = "SELECT c.id, c.vendor, c.name, c.code, c.validity_months, c.description, c.version, \
    (SELECT count(DISTINCT u.user_id) FROM wf_user_certifications u WHERE u.certification_id = c.id \
       AND (u.expires_on IS NULL OR u.expires_on >= current_date)) AS holders \
    FROM wf_certifications c";

pub async fn certifications(conn: &mut PgConnection) -> WorkforceResult<Vec<Certification>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{CERTS} WHERE c.archived_at IS NULL ORDER BY lower(c.vendor), lower(c.name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn certification(conn: &mut PgConnection, id: Uuid) -> WorkforceResult<Certification> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{CERTS} WHERE c.id = $1 AND c.archived_at IS NULL"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or(WorkforceError::NotFound)
}

struct CertFields {
    vendor: String,
    name: String,
    code: String,
    description: String,
}

fn cert_fields(input: &CertificationInput) -> WorkforceResult<CertFields> {
    if let Some(m) = input.validity_months
        && !(1..=240).contains(&m)
    {
        return Err(WorkforceError::invalid("validityMonths", "1 to 240 months"));
    }
    Ok(CertFields {
        vendor: text("vendor", Some(&input.vendor), 100)?
            .ok_or_else(|| WorkforceError::invalid("vendor", "a certification needs its vendor"))?,
        name: text("name", Some(&input.name), 200)?
            .ok_or_else(|| WorkforceError::invalid("name", "a certification needs a name"))?,
        code: text("code", input.code.as_deref(), 50)?.unwrap_or_default(),
        description: text("description", input.description.as_deref(), 2000)?.unwrap_or_default(),
    })
}

const CERT_TAKEN: &str = "there's already a certification with this vendor and name";

pub async fn create_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &CertificationInput,
) -> WorkforceResult<Certification> {
    let f = cert_fields(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO wf_certifications (id, vendor, name, code, validity_months, description) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(&f.vendor)
    .bind(&f.name)
    .bind(&f.code)
    .bind(input.validity_months)
    .bind(&f.description)
    .execute(&mut *conn)
    .await
    .map_err(|e| taken(e, "wf_certifications_name", "name", CERT_TAKEN))?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("certification.created")
            .target("certification", id)
            .details(json!({ "vendor": f.vendor, "name": f.name })),
    )
    .await?;
    certification(conn, id).await
}

pub async fn update_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &CertificationInput,
) -> WorkforceResult<Certification> {
    let f = cert_fields(input)?;
    let rows = sqlx::query(
        "UPDATE wf_certifications SET vendor = $3, name = $4, code = $5, validity_months = $6, \
                description = $7, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&f.vendor)
    .bind(&f.name)
    .bind(&f.code)
    .bind(input.validity_months)
    .bind(&f.description)
    .execute(&mut *conn)
    .await
    .map_err(|e| taken(e, "wf_certifications_name", "name", CERT_TAKEN))?
    .rows_affected();
    if rows == 0 {
        certification(conn, id).await?;
        return Err(WorkforceError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("certification.updated")
            .target("certification", id)
            .details(json!({ "vendor": f.vendor, "name": f.name })),
    )
    .await?;
    certification(conn, id).await
}

/// Archives a certification: requirements on it go; what people earned
/// stays on their record.
pub async fn archive_certification(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> WorkforceResult<()> {
    let c = certification(conn, id).await?;
    sqlx::query(
        "UPDATE wf_certifications SET archived_at = now(), version = version + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM wf_requirements WHERE certification_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("certification.archived")
            .target("certification", id)
            .details(json!({ "vendor": c.vendor, "name": c.name })),
    )
    .await?;
    Ok(())
}
