//! Sales activities (ADR-070): tasks, calls, meetings, notes and logged
//! emails, on a lead, an opportunity (and its quote) or a contact.
//!
//! An activity lives at the organization of what it's about: a lead's at
//! the provider that captured it; an opportunity's, quote's or contact's at
//! the client. Tasks have an owner and a due time, and are done or not; a
//! task given to someone else tells them.
//!
//! Emails arrive through a mailbox set to log sales email: [`log_email`]
//! files the message on each contact it's from or to (on their client's
//! latest open opportunity, if any), and on open leads with those addresses.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use huxley_notify::{Notice, kinds};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{SalesError, SalesResult, required, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = SalesActivityKind)]
pub enum Kind {
    Task,
    Call,
    Meeting,
    Note,
    Email,
}

impl Kind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Call => "call",
            Self::Meeting => "meeting",
            Self::Note => "note",
            Self::Email => "email",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesActivity)]
pub struct Activity {
    pub id: Uuid,
    pub org_id: Uuid,
    pub kind: Kind,
    pub subject: String,
    pub body: String,
    pub lead_id: Option<Uuid>,
    pub lead_name: Option<String>,
    pub opportunity_id: Option<Uuid>,
    pub opportunity_name: Option<String>,
    pub quote_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    pub contact_name: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub due_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub starts_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ends_at: Option<OffsetDateTime>,
    pub duration_minutes: Option<i32>,
    pub outcome: Option<String>,
    pub location: Option<String>,
    /// Emails: `in` (from them) or `out` (to them).
    pub direction: Option<String>,
    pub email_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub done_at: Option<OffsetDateTime>,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const SELECT: &str = "SELECT a.id, a.org_id, a.kind, a.subject, a.body, a.lead_id, \
        l.company_name AS lead_name, \
        a.opportunity_id, o.name AS opportunity_name, a.quote_id, a.contact_id, \
        nullif(btrim(coalesce(c.first_name, '') || ' ' || coalesce(c.last_name, '')), '') AS contact_name, \
        a.owner_id, u.display_name AS owner_name, a.due_at, a.starts_at, a.ends_at, a.duration_minutes, \
        a.outcome, a.location, a.direction, a.email_id, a.done_at, b.display_name AS created_by_name, a.created_at \
   FROM sales_activities a \
   LEFT JOIN sales_leads l ON l.id = a.lead_id \
   LEFT JOIN sales_opportunities o ON o.id = a.opportunity_id \
   LEFT JOIN crm_contacts c ON c.id = a.contact_id \
   LEFT JOIN users u ON u.id = a.owner_id \
   LEFT JOIN users b ON b.id = a.created_by";

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct Filter {
    pub lead_id: Option<Uuid>,
    pub opportunity_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    /// Someone's.
    pub owner_id: Option<Uuid>,
    /// Only yours (the API fills in the owner).
    #[serde(default)]
    pub mine: bool,
    /// Only tasks not yet done (and meetings to come).
    #[serde(default)]
    pub open: bool,
    pub kind: Option<Kind>,
    /// At most this many (default 100, at most 500).
    pub limit: Option<i64>,
}

/// Activities, newest first; open ones by when they're due.
pub async fn list(conn: &mut PgConnection, filter: &Filter) -> SalesResult<Vec<Activity>> {
    let order = if filter.open {
        "coalesce(a.due_at, a.starts_at) NULLS LAST, a.created_at"
    } else {
        "coalesce(a.starts_at, a.created_at) DESC, a.id DESC"
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::uuid IS NULL OR a.lead_id = $1) \
            AND ($2::uuid IS NULL OR a.opportunity_id = $2) AND ($3::uuid IS NULL OR a.contact_id = $3) \
            AND ($4::uuid IS NULL OR a.owner_id = $4) \
            AND (NOT $5 OR (a.done_at IS NULL AND (a.kind = 'task' OR (a.kind = 'meeting' AND a.starts_at > now())))) \
            AND ($6::text IS NULL OR a.kind = $6) \
          ORDER BY {order} LIMIT $7"
    )))
    .bind(filter.lead_id)
    .bind(filter.opportunity_id)
    .bind(filter.contact_id)
    .bind(filter.owner_id)
    .bind(filter.open)
    .bind(filter.kind.map(Kind::as_str))
    .bind(filter.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Activity> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(SalesError::NotFound)
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_activities WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = SalesActivityInput)]
pub struct ActivityInput {
    pub kind: Kind,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub lead_id: Option<Uuid>,
    #[serde(default)]
    pub opportunity_id: Option<Uuid>,
    #[serde(default)]
    pub quote_id: Option<Uuid>,
    #[serde(default)]
    pub contact_id: Option<Uuid>,
    /// Who it's for (default: you).
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub due_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub starts_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub ends_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub duration_minutes: Option<i32>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    /// Done already (calls, meetings and notes that happened).
    #[serde(default)]
    pub done: bool,
}

/// Where an activity on these records belongs: the opportunity's, quote's
/// or contact's client, or the lead's provider. Run in system scope (the
/// caller then checks access there).
pub async fn org_for(conn: &mut PgConnection, input: &ActivityInput) -> SalesResult<Uuid> {
    let mut orgs: Vec<Uuid> = Vec::new();
    for (sql, id) in [
        (
            "SELECT org_id FROM sales_opportunities WHERE id = $1",
            input.opportunity_id,
        ),
        (
            "SELECT org_id FROM sales_quotes WHERE id = $1",
            input.quote_id,
        ),
        (
            "SELECT org_id FROM crm_contacts WHERE id = $1",
            input.contact_id,
        ),
        (
            "SELECT org_id FROM sales_leads WHERE id = $1",
            input.lead_id,
        ),
    ] {
        let Some(id) = id else { continue };
        let org: Option<Uuid> = sqlx::query_scalar(sql)
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        orgs.push(org.ok_or(SalesError::NotFound)?);
    }
    let Some(first) = orgs.first().copied() else {
        return Err(SalesError::invalid(
            "opportunityId",
            "say what it's about: a lead, opportunity, quote or contact",
        ));
    };
    // A lead's activity stands alone; the others must agree.
    if input.lead_id.is_none() && orgs.iter().any(|o| *o != first) {
        return Err(SalesError::invalid(
            "contactId",
            "these records belong to different clients",
        ));
    }
    Ok(first)
}

struct Checked {
    subject: String,
    outcome: Option<String>,
    location: Option<String>,
}

fn check(input: &ActivityInput) -> SalesResult<Checked> {
    if input.kind == Kind::Email {
        return Err(SalesError::invalid(
            "kind",
            "emails are logged from a sales mailbox",
        ));
    }
    if input.body.chars().count() > 20000 {
        return Err(SalesError::invalid("body", "at most 20000 characters"));
    }
    if let (Some(s), Some(e)) = (input.starts_at, input.ends_at)
        && e < s
    {
        return Err(SalesError::invalid("endsAt", "ends before it starts"));
    }
    if input
        .duration_minutes
        .is_some_and(|d| !(0..=1440).contains(&d))
    {
        return Err(SalesError::invalid("durationMinutes", "0 to 1440"));
    }
    Ok(Checked {
        subject: required("subject", &input.subject, 300)?,
        outcome: text("outcome", input.outcome.as_deref(), 100)?,
        location: text("location", input.location.as_deref(), 300)?,
    })
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    input: &ActivityInput,
) -> SalesResult<Activity> {
    let c = check(input)?;
    let owner = input.owner_id.or(ctx.actor.id());
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_activities (id, org_id, kind, subject, body, lead_id, opportunity_id, quote_id, contact_id, \
            owner_id, due_at, starts_at, ends_at, duration_minutes, outcome, location, done_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
                 CASE WHEN $17 OR $3 = 'note' THEN now() END, $18)",
    )
    .bind(id)
    .bind(org_id)
    .bind(input.kind)
    .bind(&c.subject)
    .bind(input.body.trim())
    .bind(input.lead_id)
    .bind(input.opportunity_id)
    .bind(input.quote_id)
    .bind(input.contact_id)
    .bind(owner)
    .bind(input.due_at)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(input.duration_minutes)
    .bind(&c.outcome)
    .bind(&c.location)
    .bind(input.done)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    let a = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_activity.created")
            .org(org_id)
            .target("sales_activity", id)
            .details(json!({ "kind": input.kind, "subject": c.subject })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "sales.activity.created",
            org_id: Some(org_id),
            subject_type: "sales_activity",
            subject_id: id,
            payload: json!({
                "activityId": id, "kind": input.kind, "subject": c.subject, "ownerId": owner,
                "leadId": input.lead_id, "opportunityId": input.opportunity_id, "contactId": input.contact_id,
                "dueAt": input.due_at.and_then(|d| d.format(&time::format_description::well_known::Rfc3339).ok()),
            }),
        },
    )
    .await?;
    if input.kind == Kind::Task {
        tell_owner(&mut *conn, ctx, &a).await?;
    }
    Ok(a)
}

/// Tells a task's owner, when someone else gave it to them.
async fn tell_owner(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    a: &Activity,
) -> SalesResult<()> {
    let Some(owner) = a.owner_id.filter(|o| Some(*o) != ctx.actor.id()) else {
        return Ok(());
    };
    let about = a
        .opportunity_name
        .as_deref()
        .or(a.lead_name.as_deref())
        .or(a.contact_name.as_deref())
        .unwrap_or("a deal");
    let title = format!("New task: {}", a.subject);
    let body = format!("On {about}.");
    let link = match (a.opportunity_id, a.lead_id) {
        (Some(o), _) => format!("/sales/opportunities/{o}"),
        (None, Some(l)) => format!("/sales/leads/{l}"),
        _ => "/sales/activities".to_owned(),
    };
    huxley_notify::notify(
        conn,
        Notice {
            user_id: owner,
            kind: kinds::SALES_TASK,
            title: &title,
            body: &body,
            link: Some(&link),
        },
    )
    .await
    .map_err(|e| match e {
        huxley_notify::NotifyError::Db(e) => SalesError::Sqlx(e),
        other => SalesError::Conflict(other.to_string()),
    })?;
    Ok(())
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &ActivityInput,
) -> SalesResult<Activity> {
    let before = get(&mut *conn, id).await?;
    if before.kind == Kind::Email {
        return Err(SalesError::Conflict("a logged email doesn't change".into()));
    }
    let c = check(input)?;
    sqlx::query(
        "UPDATE sales_activities SET kind = $2, subject = $3, body = $4, owner_id = $5, due_at = $6, starts_at = $7, \
            ends_at = $8, duration_minutes = $9, outcome = $10, location = $11, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(input.kind)
    .bind(&c.subject)
    .bind(input.body.trim())
    .bind(input.owner_id.or(before.owner_id))
    .bind(input.due_at)
    .bind(input.starts_at)
    .bind(input.ends_at)
    .bind(input.duration_minutes)
    .bind(&c.outcome)
    .bind(&c.location)
    .execute(&mut *conn)
    .await?;
    let a = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("sales_activity.updated")
            .org(a.org_id)
            .target("sales_activity", id)
            .details(json!({ "subject": c.subject })),
    )
    .await?;
    if a.kind == Kind::Task && a.owner_id != before.owner_id {
        tell_owner(&mut *conn, ctx, &a).await?;
    }
    Ok(a)
}

/// Marks it done (or not).
pub async fn set_done(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    done: bool,
    outcome: Option<&str>,
) -> SalesResult<Activity> {
    let outcome = text("outcome", outcome, 100)?;
    let n = sqlx::query(
        "UPDATE sales_activities SET done_at = CASE WHEN $2 THEN coalesce(done_at, now()) END, \
            outcome = coalesce($3, outcome), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(done)
    .bind(&outcome)
    .execute(&mut *conn)
    .await?;
    if n.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    let a = get(&mut *conn, id).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new(if done {
            "sales_activity.done"
        } else {
            "sales_activity.reopened"
        })
        .org(a.org_id)
        .target("sales_activity", id),
    )
    .await?;
    Ok(a)
}

pub async fn delete(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> SalesResult<()> {
    let a = get(&mut *conn, id).await?;
    sqlx::query("DELETE FROM sales_activities WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("sales_activity.deleted")
            .org(a.org_id)
            .target("sales_activity", id)
            .details(json!({ "kind": a.kind, "subject": a.subject })),
    )
    .await?;
    Ok(())
}

// ---- logging email ----

/// What logging a message did.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SalesEmailLogged)]
pub struct Logged {
    /// Activities made (one per contact or lead matched).
    pub logged: usize,
    /// The addresses no contact or lead has.
    pub unmatched: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct Inbound {
    from_address: Option<String>,
    to_addresses: serde_json::Value,
    cc_addresses: serde_json::Value,
    subject: String,
    body_text: String,
    mailbox_address: String,
}

/// Files a message from a sales mailbox on each contact and open lead it's
/// from or to (other than the mailbox itself). Contacts' activities go on
/// their client's most recently changed open opportunity, if there is one.
/// Run in system scope (from the logging workflow).
pub async fn log_email(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email: Uuid,
) -> SalesResult<Logged> {
    let m: Inbound = sqlx::query_as(
        "SELECT e.from_address, e.to_addresses, e.cc_addresses, e.subject, e.body_text, mb.address AS mailbox_address \
           FROM sd_inbound_emails e JOIN sd_mailboxes mb ON mb.id = e.mailbox_id WHERE e.id = $1",
    )
    .bind(email)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(SalesError::NotFound)?;
    let them = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["address"].as_str().map(str::to_lowercase))
                    .collect()
            })
            .unwrap_or_default()
    };
    let from = m.from_address.clone().map(|f| f.to_lowercase());
    let mut to = them(&m.to_addresses);
    to.extend(them(&m.cc_addresses));
    let mut addresses: Vec<(String, &str)> = Vec::new();
    if let Some(f) = &from {
        addresses.push((f.clone(), "in"));
    }
    for t in to {
        if !addresses.iter().any(|(a, _)| *a == t) {
            addresses.push((t, "out"));
        }
    }
    addresses.retain(|(a, _)| *a != m.mailbox_address);
    let subject = if m.subject.trim().is_empty() {
        "(no subject)".to_owned()
    } else {
        m.subject.chars().take(300).collect()
    };
    let body: String = huxley_mail_body(&m.body_text);
    let mut out = Logged::default();
    for (address, direction) in &addresses {
        let contacts: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, org_id FROM crm_contacts WHERE lower(email) = $1 AND archived_at IS NULL",
        )
        .bind(address)
        .fetch_all(&mut *conn)
        .await?;
        let leads: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, org_id FROM sales_leads WHERE lower(email) = $1 AND status IN ('new', 'working')",
        )
        .bind(address)
        .fetch_all(&mut *conn)
        .await?;
        if contacts.is_empty() && leads.is_empty() {
            out.unmatched.push(address.clone());
            continue;
        }
        for (contact, org) in contacts {
            let opportunity: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM sales_opportunities WHERE org_id = $1 AND status = 'open' \
                  ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(org)
            .fetch_optional(&mut *conn)
            .await?;
            out.logged += insert_email(
                &mut *conn,
                ctx,
                email,
                org,
                &subject,
                &body,
                direction,
                (None, opportunity, Some(contact)),
            )
            .await?;
        }
        for (lead, org) in leads {
            out.logged += insert_email(
                &mut *conn,
                ctx,
                email,
                org,
                &subject,
                &body,
                direction,
                (Some(lead), None, None),
            )
            .await?;
        }
    }
    sqlx::query(
        "UPDATE sd_inbound_emails SET outcome = 'logged', outcome_detail = $2, processed_at = now() WHERE id = $1",
    )
    .bind(email)
    .bind(format!("logged on {} record(s)", out.logged))
    .execute(&mut *conn)
    .await?;
    Ok(out)
}

/// The text kept from a logged email: the reply only, at most 20000 characters.
fn huxley_mail_body(text: &str) -> String {
    text.trim().chars().take(20000).collect()
}

#[allow(clippy::too_many_arguments)]
async fn insert_email(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    email: Uuid,
    org: Uuid,
    subject: &str,
    body: &str,
    direction: &str,
    (lead, opportunity, contact): (Option<Uuid>, Option<Uuid>, Option<Uuid>),
) -> SalesResult<usize> {
    let id = huxley_core::new_id();
    let made = sqlx::query(
        "INSERT INTO sales_activities (id, org_id, kind, subject, body, lead_id, opportunity_id, contact_id, \
            direction, email_id, done_at, created_by) \
         VALUES ($1, $2, 'email', $3, $4, $5, $6, $7, $8, $9, now(), $10) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(org)
    .bind(subject)
    .bind(body)
    .bind(lead)
    .bind(opportunity)
    .bind(contact)
    .bind(direction)
    .bind(email)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    Ok(usize::try_from(made.rows_affected()).unwrap_or(0))
}
