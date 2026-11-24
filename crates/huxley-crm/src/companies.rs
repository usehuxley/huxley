//! Company details: one row per organization (ADR-035). An organization
//! without a row has the defaults (a client, active).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CrmError, CrmResult, text};

pub const RELATIONSHIPS: &[&str] = &["client", "vendor", "partner", "prospect"];
pub const STATUSES: &[&str] = &["active", "inactive", "onboarding", "offboarding"];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Company {
    pub org_id: Uuid,
    /// `client`, `vendor`, `partner` or `prospect`.
    pub relationship: String,
    /// `active`, `inactive`, `onboarding` or `offboarding`.
    pub status: String,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub notes: Option<String>,
    /// 0 until the details are first saved. Send it back when updating.
    pub version: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

impl Company {
    fn defaults(org_id: Uuid) -> Self {
        Self {
            org_id,
            relationship: "client".into(),
            status: "active".into(),
            website: None,
            phone: None,
            notes: None,
            version: 0,
            updated_at: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanyInput {
    pub relationship: String,
    pub status: String,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The version you read (0 if never saved).
    pub version: i32,
}

/// The organization's company details (defaults if never saved).
pub async fn get(conn: &mut PgConnection, org_id: Uuid) -> CrmResult<Company> {
    let row: Option<Company> = sqlx::query_as(
        "SELECT org_id, relationship, status, website, phone, notes, version, updated_at \
           FROM crm_companies WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(conn)
    .await?;
    Ok(row.unwrap_or_else(|| Company::defaults(org_id)))
}

/// Saves the company details, if `input.version` is still current.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &CompanyInput,
) -> CrmResult<Company> {
    if !RELATIONSHIPS.contains(&input.relationship.as_str()) {
        return Err(CrmError::invalid(
            "relationship",
            format!("one of {}", RELATIONSHIPS.join(", ")),
        ));
    }
    if !STATUSES.contains(&input.status.as_str()) {
        return Err(CrmError::invalid(
            "status",
            format!("one of {}", STATUSES.join(", ")),
        ));
    }
    let website = text("website", input.website.as_deref(), 500)?;
    if let Some(site) = &website
        && !(site.starts_with("https://") || site.starts_with("http://"))
    {
        return Err(CrmError::invalid("website", "an http(s):// address"));
    }
    let phone = text("phone", input.phone.as_deref(), 50)?;
    let notes = text("notes", input.notes.as_deref(), 10_000)?;
    let before = get(conn, org_id).await?;
    if before.version != input.version {
        return Err(CrmError::VersionConflict);
    }
    let changed = sqlx::query(
        "INSERT INTO crm_companies (org_id, relationship, status, website, phone, notes, version, updated_by) \
         VALUES ($1, $2, $3, $4, $5, $6, 1, $8) \
         ON CONFLICT (org_id) DO UPDATE SET relationship = $2, status = $3, website = $4, phone = $5, \
                notes = $6, version = crm_companies.version + 1, updated_at = now(), updated_by = $8 \
          WHERE crm_companies.version = $7",
    )
    .bind(org_id)
    .bind(&input.relationship)
    .bind(&input.status)
    .bind(&website)
    .bind(&phone)
    .bind(&notes)
    .bind(input.version)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(CrmError::VersionConflict);
    }
    let after = get(conn, org_id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("company.updated")
            .org(org_id)
            .target("organization", org_id)
            .details(json!({
                "before": { "relationship": before.relationship, "status": before.status },
                "after": { "relationship": after.relationship, "status": after.status },
            })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "company.updated",
            org_id: Some(org_id),
            subject_type: "organization",
            subject_id: org_id,
            payload: json!({ "relationship": after.relationship, "status": after.status }),
        },
    )
    .await?;
    Ok(after)
}

/// A company in the list: the organization and its details.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CompanySummary {
    pub org_id: Uuid,
    pub name: String,
    /// The organization's kind (`client` or `provider`).
    pub kind: String,
    pub parent_id: Option<Uuid>,
    pub relationship: String,
    pub status: String,
    pub website: Option<String>,
    pub phone: Option<String>,
    pub contacts: i64,
    pub sites: i64,
}

/// Organizations visible in the caller's transaction (its scope), as
/// companies, by name: clients only unless `providers`, matching `search`.
pub async fn list(
    conn: &mut PgConnection,
    search: Option<&str>,
    providers: bool,
) -> CrmResult<Vec<CompanySummary>> {
    let pattern = search.map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    Ok(sqlx::query_as(
        "SELECT o.id AS org_id, o.name, o.kind::text AS kind, o.parent_id, \
                coalesce(c.relationship, 'client') AS relationship, coalesce(c.status, 'active') AS status, \
                c.website, c.phone, \
                (SELECT count(*) FROM crm_contact_companies l JOIN crm_contacts k ON k.id = l.contact_id \
                  WHERE l.org_id = o.id AND k.archived_at IS NULL) AS contacts, \
                (SELECT count(*) FROM crm_sites s WHERE s.org_id = o.id AND s.archived_at IS NULL) AS sites \
           FROM organizations o LEFT JOIN crm_companies c ON c.org_id = o.id \
          WHERE o.status = 'active' AND o.parent_id IS NOT NULL \
            AND ($1 OR o.kind = 'client') \
            AND ($2::text IS NULL OR o.name ILIKE $2) \
          ORDER BY lower(o.name) LIMIT 500",
    )
    .bind(providers)
    .bind(&pattern)
    .fetch_all(conn)
    .await?)
}
