//! Contacts: people at companies (ADR-035). A contact belongs to its
//! primary company (`org_id`, which row-level security follows) and can be
//! linked to more, each link with its own job title.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{CrmError, CrmResult, text};

/// Most companies one contact can be linked to.
pub const MAX_COMPANIES: usize = 20;

/// One of a contact's companies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ContactCompany {
    pub org_id: Uuid,
    /// The company's name (ignored when setting links).
    #[serde(default)]
    pub org_name: String,
    #[serde(default)]
    pub job_title: Option<String>,
    #[serde(default)]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: Uuid,
    /// The primary company.
    pub org_id: Uuid,
    pub first_name: String,
    pub last_name: String,
    /// First and last name together.
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub mobile: Option<String>,
    /// Identities elsewhere, e.g. `{"entra": "<object id>"}`.
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub external_ids: Json,
    pub site_id: Option<Uuid>,
    #[schema(value_type = std::collections::HashMap<String, serde_json::Value>)]
    pub custom_fields: Json,
    /// Every company, the primary one first.
    #[sqlx(skip)]
    pub companies: Vec<ContactCompany>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactInput {
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub mobile: Option<String>,
    /// The job title at the primary company.
    #[serde(default)]
    pub job_title: Option<String>,
    #[serde(default)]
    #[schema(value_type = Option<std::collections::HashMap<String, String>>)]
    pub external_ids: Option<Json>,
    /// A site of the primary company.
    #[serde(default)]
    pub site_id: Option<Uuid>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

/// A page of contacts.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContactPage {
    pub items: Vec<Contact>,
    pub total: i64,
}

const COLUMNS: &str = "c.id, c.org_id, c.first_name, c.last_name, \
    btrim(c.first_name || ' ' || c.last_name) AS name, c.email, c.phone, c.mobile, c.external_ids, \
    c.site_id, c.custom_fields, c.version, c.created_at, c.updated_at, c.archived_at";

struct Checked {
    first_name: String,
    last_name: String,
    email: Option<String>,
    phone: Option<String>,
    mobile: Option<String>,
    job_title: Option<String>,
    external_ids: Json,
}

fn check(input: &ContactInput) -> CrmResult<Checked> {
    let first_name = text("firstName", Some(&input.first_name), 100)?.unwrap_or_default();
    let last_name = text("lastName", Some(&input.last_name), 100)?.unwrap_or_default();
    if first_name.is_empty() && last_name.is_empty() {
        return Err(CrmError::invalid("firstName", "a contact needs a name"));
    }
    let email = text("email", input.email.as_deref(), 320)?.map(|e| e.to_lowercase());
    if let Some(email) = &email {
        let shaped = email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
            && !email.chars().any(char::is_whitespace);
        if !shaped {
            return Err(CrmError::invalid(
                "email",
                "an email address, like ada@example.com",
            ));
        }
    }
    let external_ids = match &input.external_ids {
        None => json!({}),
        Some(Json::Object(map))
            if map.len() <= 10
                && map
                    .iter()
                    .all(|(k, v)| k.len() <= 50 && v.as_str().is_some_and(|s| s.len() <= 200)) =>
        {
            Json::Object(map.clone())
        }
        Some(_) => {
            return Err(CrmError::invalid(
                "externalIds",
                "up to 10 names and text ids, like {\"entra\": \"…\"}",
            ));
        }
    };
    Ok(Checked {
        first_name,
        last_name,
        email,
        phone: text("phone", input.phone.as_deref(), 50)?,
        mobile: text("mobile", input.mobile.as_deref(), 50)?,
        job_title: text("jobTitle", input.job_title.as_deref(), 200)?,
        external_ids,
    })
}

/// A site must belong to the contact's primary company.
async fn check_site(conn: &mut PgConnection, org_id: Uuid, site: Option<Uuid>) -> CrmResult<()> {
    let Some(site) = site else { return Ok(()) };
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM crm_sites WHERE id = $1 AND org_id = $2 AND archived_at IS NULL)",
    )
    .bind(site)
    .bind(org_id)
    .fetch_one(conn)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(CrmError::invalid(
            "siteId",
            "a site of the contact's primary company",
        ))
    }
}

async fn companies_of(conn: &mut PgConnection, id: Uuid) -> CrmResult<Vec<ContactCompany>> {
    Ok(sqlx::query_as(
        "SELECT l.org_id, o.name AS org_name, l.job_title, l.is_primary \
           FROM crm_contact_companies l JOIN organizations o ON o.id = l.org_id \
          WHERE l.contact_id = $1 ORDER BY l.is_primary DESC, lower(o.name)",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> CrmResult<Contact> {
    let mut contact: Contact = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM crm_contacts c WHERE c.id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(CrmError::NotFound)?;
    contact.companies = companies_of(conn, id).await?;
    Ok(contact)
}

/// Contacts linked to `org_id` (as their primary company or another),
/// matching `search` in name or email, by name.
pub async fn list(
    conn: &mut PgConnection,
    org_id: Uuid,
    search: Option<&str>,
    include_archived: bool,
    limit: i64,
    offset: i64,
) -> CrmResult<ContactPage> {
    let pattern = search.map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let filter = "FROM crm_contacts c JOIN crm_contact_companies l ON l.contact_id = c.id AND l.org_id = $1 \
         WHERE ($2 OR c.archived_at IS NULL) \
           AND ($3::text IS NULL OR (c.first_name || ' ' || c.last_name) ILIKE $3 OR c.email ILIKE $3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {filter}")))
        .bind(org_id)
        .bind(include_archived)
        .bind(&pattern)
        .fetch_one(&mut *conn)
        .await?;
    let mut items: Vec<Contact> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} {filter} ORDER BY lower(c.last_name), lower(c.first_name), c.id \
          LIMIT $4 OFFSET $5"
    )))
    .bind(org_id)
    .bind(include_archived)
    .bind(&pattern)
    .bind(limit.clamp(1, 200))
    .bind(offset.max(0))
    .fetch_all(&mut *conn)
    .await?;
    for contact in &mut items {
        contact.companies = companies_of(conn, contact.id).await?;
    }
    Ok(ContactPage { items, total })
}

/// The contact linked to `org_id` with this email, or with this external
/// id (`(system, id)`), if there is one. Archived contacts don't count.
pub async fn find_one(
    conn: &mut PgConnection,
    org_id: Uuid,
    email: Option<&str>,
    external: Option<(&str, &str)>,
) -> CrmResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT c.id FROM crm_contacts c \
           JOIN crm_contact_companies l ON l.contact_id = c.id AND l.org_id = $1 \
          WHERE c.archived_at IS NULL \
            AND (($2::text IS NOT NULL AND lower(c.email) = lower($2)) \
              OR ($3::text IS NOT NULL AND c.external_ids ->> $3 = $4)) \
          ORDER BY c.created_at LIMIT 1",
    )
    .bind(org_id)
    .bind(email)
    .bind(external.map(|(system, _)| system))
    .bind(external.map(|(_, id)| id))
    .fetch_optional(conn)
    .await?)
}

/// Contacts visible in the transaction's scope with this email and/or
/// external id (`(system, id)`), oldest first, at most `limit`.
pub async fn search(
    conn: &mut PgConnection,
    email: Option<&str>,
    external: Option<(&str, &str)>,
    limit: i64,
) -> CrmResult<Vec<Contact>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM crm_contacts WHERE archived_at IS NULL \
            AND ($1::text IS NULL OR lower(email) = lower($1)) \
            AND ($2::text IS NULL OR external_ids ->> $2 = $3) \
          ORDER BY created_at LIMIT $4",
    )
    .bind(email)
    .bind(external.map(|(system, _)| system))
    .bind(external.map(|(_, id)| id))
    .bind(limit.clamp(1, 100))
    .fetch_all(&mut *conn)
    .await?;
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        found.push(get(conn, id).await?);
    }
    Ok(found)
}

/// Contacts with this email address (for matching mail to people).
pub async fn find_by_email(conn: &mut PgConnection, email: &str) -> CrmResult<Vec<Contact>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM crm_contacts WHERE lower(email) = lower($1) AND archived_at IS NULL \
          ORDER BY created_at LIMIT 20",
    )
    .bind(email.trim())
    .fetch_all(&mut *conn)
    .await?;
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        found.push(get(conn, id).await?);
    }
    Ok(found)
}

async fn record(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    contact: &Contact,
    action: &str,
) -> CrmResult<()> {
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action)
            .org(contact.org_id)
            .target("contact", contact.id)
            .details(json!({ "name": contact.name })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: action,
            org_id: Some(contact.org_id),
            subject_type: "contact",
            subject_id: contact.id,
            payload: json!({ "name": contact.name, "email": contact.email }),
        },
    )
    .await?;
    Ok(())
}

/// Creates a contact whose primary company is `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &ContactInput,
) -> CrmResult<Contact> {
    let c = check(input)?;
    check_site(conn, org_id, input.site_id).await?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO crm_contacts (id, org_id, first_name, last_name, email, phone, mobile, external_ids, site_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.first_name)
    .bind(&c.last_name)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.mobile)
    .bind(&c.external_ids)
    .bind(input.site_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO crm_contact_companies (contact_id, org_id, job_title, is_primary) VALUES ($1, $2, $3, true)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.job_title)
    .execute(&mut *conn)
    .await?;
    let contact = get(conn, id).await?;
    record(conn, ctx, &contact, "contact.created").await?;
    Ok(contact)
}

/// Updates a contact's details (not its companies: see [`set_companies`]).
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ContactInput,
) -> CrmResult<Contact> {
    let c = check(input)?;
    let before = get(conn, id).await?;
    let version = input.version.unwrap_or(-1);
    if before.version != version {
        return Err(CrmError::VersionConflict);
    }
    check_site(conn, before.org_id, input.site_id).await?;
    let changed = sqlx::query(
        "UPDATE crm_contacts SET first_name = $3, last_name = $4, email = $5, phone = $6, mobile = $7, \
                external_ids = $8, site_id = $9, version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&c.first_name)
    .bind(&c.last_name)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.mobile)
    .bind(&c.external_ids)
    .bind(input.site_id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(CrmError::VersionConflict);
    }
    sqlx::query(
        "UPDATE crm_contact_companies SET job_title = $2 WHERE contact_id = $1 AND is_primary",
    )
    .bind(id)
    .bind(&c.job_title)
    .execute(&mut *conn)
    .await?;
    let contact = get(conn, id).await?;
    record(conn, ctx, &contact, "contact.updated").await?;
    Ok(contact)
}

/// Replaces a contact's companies. Exactly one is primary; it becomes the
/// contact's `org_id` (a site of another company is cleared). The caller
/// checks the person may manage contacts at every company involved.
pub async fn set_companies(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
    links: &[ContactCompany],
) -> CrmResult<Contact> {
    let primaries: Vec<&ContactCompany> = links.iter().filter(|l| l.is_primary).collect();
    if primaries.len() != 1 {
        return Err(CrmError::invalid(
            "companies",
            "exactly one company is the primary one",
        ));
    }
    if links.len() > MAX_COMPANIES {
        return Err(CrmError::invalid(
            "companies",
            format!("at most {MAX_COMPANIES}"),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if !links.iter().all(|l| seen.insert(l.org_id)) {
        return Err(CrmError::invalid("companies", "each company once"));
    }
    let titles = links
        .iter()
        .map(|l| text("jobTitle", l.job_title.as_deref(), 200))
        .collect::<CrmResult<Vec<_>>>()?;
    let before = get(conn, id).await?;
    if before.version != version {
        return Err(CrmError::VersionConflict);
    }
    let primary = primaries[0].org_id;
    sqlx::query("DELETE FROM crm_contact_companies WHERE contact_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for (link, title) in links.iter().zip(titles) {
        sqlx::query(
            "INSERT INTO crm_contact_companies (contact_id, org_id, job_title, is_primary) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(link.org_id)
        .bind(title)
        .bind(link.is_primary)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "UPDATE crm_contacts SET org_id = $2, version = version + 1, updated_at = now(), \
                site_id = CASE WHEN org_id = $2 THEN site_id END \
          WHERE id = $1",
    )
    .bind(id)
    .bind(primary)
    .execute(&mut *conn)
    .await?;
    let contact = get(conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("contact.companies_changed")
            .org(primary)
            .target("contact", id)
            .details(json!({
                "before": before.companies.iter().map(|c| c.org_id).collect::<Vec<_>>(),
                "after": contact.companies.iter().map(|c| c.org_id).collect::<Vec<_>>(),
                "primary": primary,
            })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: "contact.updated",
            org_id: Some(primary),
            subject_type: "contact",
            subject_id: id,
            payload: json!({ "name": contact.name, "email": contact.email }),
        },
    )
    .await?;
    Ok(contact)
}

pub async fn archive(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> CrmResult<Contact> {
    let changed = sqlx::query(
        "UPDATE crm_contacts SET archived_at = now(), version = version + 1 \
          WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(CrmError::NotFound);
    }
    let contact = get(conn, id).await?;
    record(conn, ctx, &contact, "contact.archived").await?;
    Ok(contact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contacts_need_a_name_and_a_real_email() {
        let named = ContactInput {
            first_name: " Ada ".into(),
            email: Some(" Ada@Example.COM ".into()),
            ..ContactInput::default()
        };
        let checked = check(&named).unwrap();
        assert_eq!(checked.first_name, "Ada");
        assert_eq!(checked.email.as_deref(), Some("ada@example.com"));
        assert!(check(&ContactInput::default()).is_err());
        for bad in [
            "ada",
            "ada@",
            "@example.com",
            "a da@example.com",
            "ada@localhost",
        ] {
            let input = ContactInput {
                last_name: "Lovelace".into(),
                email: Some(bad.into()),
                ..ContactInput::default()
            };
            assert!(check(&input).is_err(), "{bad}");
        }
        let ids = ContactInput {
            last_name: "L".into(),
            external_ids: Some(json!({ "entra": 5 })),
            ..ContactInput::default()
        };
        assert!(check(&ids).is_err());
    }
}
