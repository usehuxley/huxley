//! Leads: someone who may buy, before there's a deal.
//!
//! - Held at the provider organization that captured it; captured by
//!   people, forms or the API.
//! - **Converting** one makes the company (a new client organization under
//!   the lead's, with relationship "prospect", or an existing client), the
//!   contact from the lead's person, and an opportunity. The lead keeps
//!   links to all three.
//! - Raises `sales.lead.created` and `sales.lead.converted`.

use huxley_core::RequestContext;
use huxley_crm::contacts::{self, ContactInput};
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_orgs::OrgKind;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::opportunities::{self, Opportunity, OpportunityInput};
use crate::{SalesError, SalesResult, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = LeadStatus)]
pub enum Status {
    New,
    Working,
    Disqualified,
    Converted,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Lead {
    pub id: Uuid,
    /// The provider organization that captured it.
    pub org_id: Uuid,
    pub company_name: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub source: Option<String>,
    pub notes: String,
    pub status: Status,
    pub disqualified_reason: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub converted_org_id: Option<Uuid>,
    pub converted_contact_id: Option<Uuid>,
    pub converted_opportunity_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub converted_at: Option<OffsetDateTime>,
    pub version: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeadInput {
    pub company_name: String,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    /// `new` or `working` (disqualify or convert to change otherwise).
    #[serde(default)]
    pub status: Option<Status>,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub status: Option<Status>,
    pub owner_id: Option<Uuid>,
    /// In the company, the person or the email.
    pub q: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConvertInput {
    /// An existing client to attach it to; otherwise a new one is made from
    /// the lead's company name, under the lead's organization.
    #[serde(default)]
    pub org_id: Option<Uuid>,
    /// Make a contact from the lead's person (default yes, if it has a name).
    #[serde(default = "yes")]
    pub create_contact: bool,
    pub opportunity: OpportunityInput,
    pub version: i32,
}

fn yes() -> bool {
    true
}

/// What converting made.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub lead: Lead,
    pub opportunity: Opportunity,
    pub contact_id: Option<Uuid>,
}

const SELECT: &str = "SELECT l.id, l.org_id, l.company_name, l.first_name, l.last_name, l.email, \
        l.phone, l.source, l.notes, l.status, l.disqualified_reason, l.owner_id, \
        u.display_name AS owner_name, l.converted_org_id, l.converted_contact_id, \
        l.converted_opportunity_id, l.converted_at, l.version, l.created_at, l.updated_at \
   FROM sales_leads l LEFT JOIN users u ON u.id = l.owner_id";

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Lead> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE l.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(SalesError::NotFound)
}

/// Leads you can see, newest first (at most 1000).
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> SalesResult<Vec<Lead>> {
    let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR l.status = $1) \
            AND ($2::uuid IS NULL OR l.owner_id = $2) \
            AND ($3::text IS NULL OR l.company_name ILIKE '%' || $3 || '%' \
                 OR l.email ILIKE '%' || $3 || '%' \
                 OR (coalesce(l.first_name, '') || ' ' || coalesce(l.last_name, '')) ILIKE '%' || $3 || '%') \
          ORDER BY l.created_at DESC LIMIT 1000"
    )))
    .bind(filter.status)
    .bind(filter.owner_id)
    .bind(q)
    .fetch_all(conn)
    .await?)
}

struct Checked {
    company_name: String,
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    source: Option<String>,
}

fn check(input: &LeadInput) -> SalesResult<Checked> {
    let email = text("email", input.email.as_deref(), 254)?;
    if email
        .as_deref()
        .is_some_and(|e| !e.contains('@') || e.contains(char::is_whitespace))
    {
        return Err(SalesError::invalid("email", "not an email address"));
    }
    if input.notes.chars().count() > 8000 {
        return Err(SalesError::invalid("notes", "at most 8000 characters"));
    }
    if matches!(input.status, Some(Status::Disqualified | Status::Converted)) {
        return Err(SalesError::invalid(
            "status",
            "disqualify or convert it instead",
        ));
    }
    Ok(Checked {
        company_name: required("companyName", &input.company_name, 200)?,
        first_name: text("firstName", input.first_name.as_deref(), 100)?,
        last_name: text("lastName", input.last_name.as_deref(), 100)?,
        email,
        phone: text("phone", input.phone.as_deref(), 50)?,
        source: text("source", input.source.as_deref(), 60)?,
    })
}

async fn emit(conn: &mut PgConnection, event: &'static str, lead: &Lead) -> SalesResult<()> {
    outbox::emit(
        conn,
        DomainEvent {
            event_type: event,
            org_id: Some(lead.org_id),
            subject_type: "lead",
            subject_id: lead.id,
            payload: json!({
                "leadId": lead.id,
                "companyName": lead.company_name,
                "email": lead.email,
                "source": lead.source,
                "status": lead.status,
                "opportunityId": lead.converted_opportunity_id,
                "clientId": lead.converted_org_id,
            }),
        },
    )
    .await?;
    Ok(())
}

/// Captures a lead at the provider organization `org_id`.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &LeadInput,
) -> SalesResult<Lead> {
    let c = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_leads (id, org_id, company_name, first_name, last_name, email, phone, \
            source, notes, status, owner_id, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(org_id)
    .bind(&c.company_name)
    .bind(&c.first_name)
    .bind(&c.last_name)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.source)
    .bind(&input.notes)
    .bind(input.status.unwrap_or(Status::New))
    .bind(input.owner_id)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let lead = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("lead.created")
            .org(org_id)
            .target("lead", id)
            .details(json!({ "company": lead.company_name, "source": lead.source })),
    )
    .await?;
    emit(&mut *conn, "sales.lead.created", &lead).await?;
    Ok(lead)
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &LeadInput,
) -> SalesResult<Lead> {
    let c = check(input)?;
    let current = get(&mut *conn, id).await?;
    if current.status == Status::Converted {
        return Err(SalesError::Conflict(
            "it's converted; change its opportunity instead".into(),
        ));
    }
    let version = input
        .version
        .ok_or_else(|| SalesError::invalid("version", "send the version you read"))?;
    let done = sqlx::query(
        "UPDATE sales_leads SET company_name = $3, first_name = $4, last_name = $5, email = $6, \
            phone = $7, source = $8, notes = $9, owner_id = $10, \
            status = CASE WHEN $11::text IS NULL THEN status ELSE $11 END, \
            disqualified_reason = CASE WHEN $11::text IS NULL THEN disqualified_reason ELSE NULL END, \
            version = version + 1, updated_at = now() \
          WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&c.company_name)
    .bind(&c.first_name)
    .bind(&c.last_name)
    .bind(&c.email)
    .bind(&c.phone)
    .bind(&c.source)
    .bind(&input.notes)
    .bind(input.owner_id)
    .bind(input.status)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::VersionConflict);
    }
    let lead = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("lead.updated")
            .org(lead.org_id)
            .target("lead", id)
            .details(json!({ "company": lead.company_name, "status": lead.status })),
    )
    .await?;
    Ok(lead)
}

/// Marks it not worth pursuing, saying why.
pub async fn disqualify(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    reason: &str,
    version: i32,
) -> SalesResult<Lead> {
    let reason = required("reason", reason, 500)?;
    let current = get(&mut *conn, id).await?;
    if current.status == Status::Converted {
        return Err(SalesError::Conflict("it's already converted".into()));
    }
    let done = sqlx::query(
        "UPDATE sales_leads SET status = 'disqualified', disqualified_reason = $3, \
            version = version + 1, updated_at = now() WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .bind(&reason)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::VersionConflict);
    }
    let lead = get(&mut *conn, id).await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("lead.disqualified")
            .org(lead.org_id)
            .target("lead", id)
            .details(json!({ "reason": reason })),
    )
    .await?;
    Ok(lead)
}

/// The client a lead converts into: an existing one, or a new prospect
/// under the lead's organization.
async fn client_for(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    lead: &Lead,
    existing: Option<Uuid>,
) -> SalesResult<Uuid> {
    if let Some(org) = existing {
        let found = huxley_orgs::get(&mut *conn, org).await?;
        if found.kind != OrgKind::Client {
            return Err(SalesError::invalid("orgId", "not a client organization"));
        }
        return Ok(org);
    }
    let org = huxley_orgs::create(
        &mut *conn,
        ctx,
        lead.org_id,
        OrgKind::Client,
        &lead.company_name,
    )
    .await
    .map_err(|e| match e {
        huxley_orgs::OrgError::NameTaken => SalesError::Conflict(format!(
            "{} is already a client here; convert into it instead",
            lead.company_name
        )),
        e => e.into(),
    })?;
    sqlx::query(
        "INSERT INTO crm_companies (org_id, relationship) VALUES ($1, 'prospect') \
         ON CONFLICT (org_id) DO UPDATE SET relationship = 'prospect'",
    )
    .bind(org.id)
    .execute(&mut *conn)
    .await?;
    Ok(org.id)
}

/// Turns a lead into a client (new or existing), a contact and an
/// opportunity, all in the caller's transaction.
pub async fn convert(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ConvertInput,
) -> SalesResult<Converted> {
    let lead = get(&mut *conn, id).await?;
    if lead.version != input.version {
        return Err(SalesError::VersionConflict);
    }
    match lead.status {
        Status::Converted => return Err(SalesError::Conflict("it's already converted".into())),
        Status::Disqualified => {
            return Err(SalesError::Conflict(
                "it's disqualified; set it back to working first".into(),
            ));
        }
        Status::New | Status::Working => {}
    }
    let org_id = client_for(&mut *conn, ctx, &lead, input.org_id).await?;
    let contact_id =
        if input.create_contact && (lead.first_name.is_some() || lead.last_name.is_some()) {
            let contact = contacts::create(
                &mut *conn,
                ctx,
                org_id,
                &ContactInput {
                    first_name: lead.first_name.clone().unwrap_or_default(),
                    last_name: lead.last_name.clone().unwrap_or_default(),
                    email: lead.email.clone(),
                    phone: lead.phone.clone(),
                    mobile: None,
                    job_title: None,
                    external_ids: None,
                    site_id: None,
                    version: None,
                },
            )
            .await?;
            Some(contact.id)
        } else {
            None
        };
    let mut opportunity = input.opportunity.clone();
    if opportunity.contact_id.is_none() {
        opportunity.contact_id = contact_id;
    }
    if opportunity.owner_id.is_none() {
        opportunity.owner_id = lead.owner_id;
    }
    if opportunity.source.is_none() {
        opportunity.source.clone_from(&lead.source);
    }
    let opportunity =
        opportunities::create(&mut *conn, ctx, org_id, &opportunity, Some(id)).await?;
    sqlx::query(
        "UPDATE sales_leads SET status = 'converted', converted_org_id = $2, converted_contact_id = $3, \
            converted_opportunity_id = $4, converted_at = now(), version = version + 1, updated_at = now() \
          WHERE id = $1",
    )
    .bind(id)
    .bind(org_id)
    .bind(contact_id)
    .bind(opportunity.id)
    .execute(&mut *conn)
    .await?;
    let lead = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("lead.converted")
            .org(lead.org_id)
            .target("lead", id)
            .details(json!({ "clientId": org_id, "opportunityId": opportunity.id, "contactId": contact_id })),
    )
    .await?;
    emit(&mut *conn, "sales.lead.converted", &lead).await?;
    Ok(Converted {
        lead,
        opportunity,
        contact_id,
    })
}

/// Deletes a lead that hasn't been converted.
pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<()> {
    let lead = get(&mut *conn, id).await?;
    if lead.status == Status::Converted {
        return Err(SalesError::Conflict(
            "it's converted; its records stay".into(),
        ));
    }
    sqlx::query("DELETE FROM sales_leads WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("lead.deleted")
            .org(lead.org_id)
            .target("lead", id)
            .details(json!({ "company": lead.company_name })),
    )
    .await?;
    Ok(())
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_leads WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}
