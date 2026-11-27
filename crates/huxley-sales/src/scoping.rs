//! Presales scoping (ADR-072): a workspace on an opportunity.
//!
//! - **Discovery**: notes, call transcripts and documents' text, plus what
//!   Huxley knows about the client (its people, sites and devices, its
//!   agreements) and similar past work.
//! - **Scope of Work**: sections from a template per kind of engagement.
//!   The Presales scoping pack drafts them with the organization's AI
//!   provider ([`context`] gathers what it reads; [`write_draft`] keeps what
//!   it wrote), citing its sources and turning gaps into open questions. It
//!   never touches a section someone has edited.
//! - **Approving** keeps a numbered version; a quote can carry an approved
//!   version, which is then signed with it.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{SalesError, SalesResult, required, text};

// ---- templates ----

/// A kind of engagement, and the sections its Scope of Work has.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeTemplate)]
pub struct Template {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    #[schema(value_type = Vec<TemplateSection>)]
    pub sections: &'static [TemplateSection],
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeTemplateSection)]
pub struct TemplateSection {
    pub key: &'static str,
    pub title: &'static str,
    /// What belongs in it (for people, and for the AI).
    pub guidance: &'static str,
}

const fn section(
    key: &'static str,
    title: &'static str,
    guidance: &'static str,
) -> TemplateSection {
    TemplateSection {
        key,
        title,
        guidance,
    }
}

const OBJECTIVES: TemplateSection = section(
    "objectives",
    "Objectives",
    "Why the client wants this: the business outcomes, in their words where possible.",
);
const IN_SCOPE: TemplateSection = section(
    "in_scope",
    "In scope",
    "What will be done, concretely: systems, sites, users and devices covered, with counts.",
);
const OUT_OF_SCOPE: TemplateSection = section(
    "out_of_scope",
    "Out of scope",
    "What won't be done, especially things the client might assume are included.",
);
const DELIVERABLES: TemplateSection = section(
    "deliverables",
    "Deliverables",
    "What the client gets at the end: each deliverable as a short, checkable item.",
);
const ASSUMPTIONS: TemplateSection = section(
    "assumptions",
    "Assumptions",
    "What the plan and price depend on being true; each one a risk if it isn't.",
);
const CLIENT_RESPONSIBILITIES: TemplateSection = section(
    "client_responsibilities",
    "Client responsibilities",
    "What the client must provide or do, and by when: access, people, decisions, licences.",
);
const ACCEPTANCE: TemplateSection = section(
    "acceptance",
    "Acceptance criteria",
    "How both sides will agree the work is done: tests, sign-offs, measures.",
);
const TIMELINE: TemplateSection = section(
    "timeline",
    "Timeline",
    "The phases and milestones in order, with rough durations; no firm dates unless discovery gives them.",
);
const CHANGE_CONTROL: TemplateSection = section(
    "change_control",
    "Change control",
    "How changes to scope are requested, priced and approved.",
);

pub const TEMPLATES: &[Template] = &[
    Template {
        key: "project",
        name: "Project",
        description: "A piece of work with a start and an end: a rollout, an upgrade, a refresh.",
        sections: &[
            OBJECTIVES,
            IN_SCOPE,
            OUT_OF_SCOPE,
            DELIVERABLES,
            ASSUMPTIONS,
            CLIENT_RESPONSIBILITIES,
            ACCEPTANCE,
            TIMELINE,
            CHANGE_CONTROL,
        ],
    },
    Template {
        key: "managed_services",
        name: "Managed services onboarding",
        description: "Taking over a client's IT: discovery, documentation, tooling and handover.",
        sections: &[
            OBJECTIVES,
            IN_SCOPE,
            OUT_OF_SCOPE,
            section(
                "service_levels",
                "Service levels",
                "Response and resolution targets, hours of cover, and how they're measured.",
            ),
            DELIVERABLES,
            ASSUMPTIONS,
            CLIENT_RESPONSIBILITIES,
            section(
                "onboarding",
                "Onboarding plan",
                "The steps to take over: discovery, agents, documentation, access, go-live.",
            ),
            CHANGE_CONTROL,
        ],
    },
    Template {
        key: "migration",
        name: "Migration",
        description: "Moving users, data or systems from one platform to another.",
        sections: &[
            OBJECTIVES,
            IN_SCOPE,
            OUT_OF_SCOPE,
            section(
                "cutover",
                "Cutover and rollback",
                "How and when the switch happens, what users notice, and how it's undone if it fails.",
            ),
            DELIVERABLES,
            ASSUMPTIONS,
            CLIENT_RESPONSIBILITIES,
            ACCEPTANCE,
            TIMELINE,
            CHANGE_CONTROL,
        ],
    },
    Template {
        key: "assessment",
        name: "Assessment",
        description: "A review that ends in findings and recommendations: security, network, cloud.",
        sections: &[
            OBJECTIVES,
            IN_SCOPE,
            OUT_OF_SCOPE,
            section(
                "approach",
                "Approach",
                "How the assessment is done: interviews, scans, reviews, and what access is needed.",
            ),
            DELIVERABLES,
            ASSUMPTIONS,
            CLIENT_RESPONSIBILITIES,
            TIMELINE,
        ],
    },
];

pub fn template(key: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.key == key)
}

// ---- the workspace ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = ScopeDraftState)]
pub enum DraftState {
    Idle,
    /// Waiting for the drafting workflow.
    Requested,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeSource)]
pub struct Source {
    pub id: Uuid,
    /// `note`, `transcript` or `document`.
    pub kind: String,
    pub title: String,
    pub body: String,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeCitation)]
pub struct Citation {
    pub label: String,
    /// `note`, `transcript`, `document`, `environment`, `agreements` or `past_work`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeSection)]
pub struct Section {
    pub key: String,
    pub title: String,
    pub body: String,
    #[sqlx(json)]
    pub citations: Vec<Citation>,
    /// The text is the AI's, untouched since.
    pub drafted: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeQuestion)]
pub struct Question {
    pub id: Uuid,
    pub section_key: Option<String>,
    pub question: String,
    pub answer: Option<String>,
    pub from_ai: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeVersion)]
pub struct Version {
    pub id: Uuid,
    pub number: i32,
    pub created_by_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub id: Uuid,
    pub org_id: Uuid,
    pub opportunity_id: Uuid,
    pub opportunity_name: String,
    pub template: String,
    pub draft_state: DraftState,
    pub draft_error: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub draft_requested_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub drafted_at: Option<OffsetDateTime>,
    pub version: i32,
    #[sqlx(skip)]
    pub sources: Vec<Source>,
    #[sqlx(skip)]
    pub sections: Vec<Section>,
    #[sqlx(skip)]
    pub questions: Vec<Question>,
    #[sqlx(skip)]
    pub versions: Vec<Version>,
}

/// The scope of an opportunity, if it has one.
pub async fn of_opportunity(
    conn: &mut PgConnection,
    opportunity: Uuid,
) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM sales_scopes WHERE opportunity_id = $1")
            .bind(opportunity)
            .fetch_optional(conn)
            .await?,
    )
}

pub async fn org_of(conn: &mut PgConnection, id: Uuid) -> SalesResult<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM sales_scopes WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> SalesResult<Scope> {
    let mut s: Scope = sqlx::query_as(
        "SELECT s.id, s.org_id, s.opportunity_id, o.name AS opportunity_name, s.template, s.draft_state, \
                s.draft_error, s.draft_requested_at, s.drafted_at, s.version \
           FROM sales_scopes s JOIN sales_opportunities o ON o.id = s.opportunity_id WHERE s.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(SalesError::NotFound)?;
    s.sources = sqlx::query_as(
        "SELECT x.id, x.kind, x.title, x.body, u.display_name AS created_by_name, x.created_at \
           FROM sales_scope_sources x LEFT JOIN users u ON u.id = x.created_by \
          WHERE x.scope_id = $1 ORDER BY x.created_at, x.id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    s.sections = sqlx::query_as(
        "SELECT key, title, body, citations, drafted, updated_at FROM sales_scope_sections \
          WHERE scope_id = $1 ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    s.questions = sqlx::query_as(
        "SELECT id, section_key, question, answer, from_ai, resolved_at FROM sales_scope_questions \
          WHERE scope_id = $1 ORDER BY resolved_at NULLS FIRST, created_at",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    s.versions = sqlx::query_as(
        "SELECT v.id, v.number, u.display_name AS created_by_name, v.created_at FROM sales_scope_versions v \
           LEFT JOIN users u ON u.id = v.created_by WHERE v.scope_id = $1 ORDER BY v.number DESC",
    )
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(s)
}

/// Starts scoping an opportunity with a template (or returns its scope).
pub async fn open(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    opportunity: Uuid,
    template_key: &str,
) -> SalesResult<Scope> {
    if let Some(id) = of_opportunity(&mut *conn, opportunity).await? {
        return get(conn, id).await;
    }
    let t = template(template_key)
        .ok_or_else(|| SalesError::invalid("template", "not a scoping template"))?;
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM sales_opportunities WHERE id = $1")
        .bind(opportunity)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(SalesError::NotFound)?;
    let id = huxley_core::new_id();
    sqlx::query("INSERT INTO sales_scopes (id, org_id, opportunity_id, template, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(org)
        .bind(opportunity)
        .bind(t.key)
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
    for (i, sec) in t.sections.iter().enumerate() {
        sqlx::query(
            "INSERT INTO sales_scope_sections (id, scope_id, org_id, key, title, position) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(huxley_core::new_id())
        .bind(id)
        .bind(org)
        .bind(sec.key)
        .bind(sec.title)
        .bind(i32::try_from(i).unwrap_or(0))
        .execute(&mut *conn)
        .await?;
    }
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.created")
            .org(org)
            .target("scope", id)
            .details(json!({ "opportunity": opportunity, "template": t.key })),
    )
    .await?;
    get(conn, id).await
}

async fn touch(conn: &mut PgConnection, id: Uuid) -> SalesResult<()> {
    sqlx::query("UPDATE sales_scopes SET version = version + 1, updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ScopeSourceInput)]
pub struct SourceInput {
    /// `note`, `transcript` or `document`.
    pub kind: String,
    pub title: String,
    /// The text (a document's, pasted or extracted).
    pub body: String,
}

pub async fn add_source(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &SourceInput,
) -> SalesResult<Scope> {
    if !matches!(input.kind.as_str(), "note" | "transcript" | "document") {
        return Err(SalesError::invalid("kind", "note, transcript or document"));
    }
    let title = required("title", &input.title, 200)?;
    let body = required("body", &input.body, 100_000)?;
    let org = org_of(&mut *conn, id).await?.ok_or(SalesError::NotFound)?;
    sqlx::query(
        "INSERT INTO sales_scope_sources (id, scope_id, org_id, kind, title, body, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(org)
    .bind(&input.kind)
    .bind(&title)
    .bind(&body)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    touch(&mut *conn, id).await?;
    get(conn, id).await
}

pub async fn delete_source(conn: &mut PgConnection, id: Uuid, source: Uuid) -> SalesResult<Scope> {
    sqlx::query("DELETE FROM sales_scope_sources WHERE id = $1 AND scope_id = $2")
        .bind(source)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    touch(&mut *conn, id).await?;
    get(conn, id).await
}

/// Changes a section's text (it's then the person's, not the AI's).
pub async fn edit_section(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    key: &str,
    body: &str,
) -> SalesResult<Scope> {
    if body.chars().count() > 20000 {
        return Err(SalesError::invalid("body", "at most 20000 characters"));
    }
    let done = sqlx::query(
        "UPDATE sales_scope_sections SET body = $3, drafted = false, updated_by = $4, updated_at = now() \
          WHERE scope_id = $1 AND key = $2",
    )
    .bind(id)
    .bind(key)
    .bind(body.trim())
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    touch(&mut *conn, id).await?;
    get(conn, id).await
}

/// Asks (or answers and resolves, or reopens) an open question.
pub async fn add_question(
    conn: &mut PgConnection,
    id: Uuid,
    section: Option<&str>,
    question: &str,
) -> SalesResult<Scope> {
    let question = required("question", question, 1000)?;
    let org = org_of(&mut *conn, id).await?.ok_or(SalesError::NotFound)?;
    sqlx::query(
        "INSERT INTO sales_scope_questions (id, scope_id, org_id, section_key, question) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(huxley_core::new_id())
    .bind(id)
    .bind(org)
    .bind(section)
    .bind(&question)
    .execute(&mut *conn)
    .await?;
    get(conn, id).await
}

pub async fn answer_question(
    conn: &mut PgConnection,
    id: Uuid,
    question: Uuid,
    answer: Option<&str>,
    resolved: bool,
) -> SalesResult<Scope> {
    let answer = text("answer", answer, 4000)?;
    let done = sqlx::query(
        "UPDATE sales_scope_questions SET answer = coalesce($3, answer), \
            resolved_at = CASE WHEN $4 THEN coalesce(resolved_at, now()) END WHERE id = $1 AND scope_id = $2",
    )
    .bind(question)
    .bind(id)
    .bind(&answer)
    .bind(resolved)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(SalesError::NotFound);
    }
    get(conn, id).await
}

// ---- drafting ----

/// Asks the drafting workflow for a draft: raises `scope.draft_requested`.
pub async fn request_draft(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
) -> SalesResult<Scope> {
    let s = get(&mut *conn, id).await?;
    if s.sources.is_empty() {
        return Err(SalesError::invalid(
            "sources",
            "add some discovery first: notes, a transcript or a document",
        ));
    }
    sqlx::query(
        "UPDATE sales_scopes SET draft_state = 'requested', draft_error = NULL, draft_requested_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "scope.draft_requested",
            org_id: Some(s.org_id),
            subject_type: "scope",
            subject_id: id,
            payload: json!({ "scopeId": id, "opportunityId": s.opportunity_id, "template": s.template,
                             "requestedBy": ctx.actor.id() }),
        },
    )
    .await?;
    get(conn, id).await
}

/// Stops waiting for a draft (say, the pack isn't installed).
pub async fn cancel_draft(conn: &mut PgConnection, id: Uuid) -> SalesResult<Scope> {
    sqlx::query("UPDATE sales_scopes SET draft_state = 'idle' WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    get(conn, id).await
}

/// What the drafting workflow reads: the discovery, numbered for citing.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeContext)]
pub struct Context {
    /// Everything, as text for the AI: numbered sources [1], [2]…
    pub text: String,
    /// The sections to write: `key: title: guidance` (those not edited by people).
    pub sections: Vec<String>,
    /// What each source number stands for.
    #[schema(value_type = Vec<Citation>)]
    pub sources: Vec<Citation>,
}

/// Gathers what the drafting workflow reads: the opportunity, the discovery
/// sources, the client's environment and agreements, and approved scopes of
/// past won work on the same template. Run in system scope.
pub async fn context(conn: &mut PgConnection, id: Uuid) -> SalesResult<Context> {
    let s = get(&mut *conn, id).await?;
    let t = template(&s.template).ok_or_else(|| SalesError::Conflict("unknown template".into()))?;
    let (client, description, amount, recurring): (
        String,
        String,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    ) = sqlx::query_as(
        "SELECT o.name, op.description, op.amount, op.recurring FROM sales_opportunities op \
               JOIN organizations o ON o.id = op.org_id WHERE op.id = $1",
    )
    .bind(s.opportunity_id)
    .fetch_one(&mut *conn)
    .await?;
    let mut parts = vec![format!(
        "Engagement: {} ({}).\nClient: {client}.\nOpportunity: {}. {description}\nValue so far: {} one-time, {} a month.",
        t.name,
        t.description,
        s.opportunity_name,
        amount.normalize(),
        recurring.normalize()
    )];
    let mut sources: Vec<Citation> = Vec::new();
    for src in &s.sources {
        sources.push(Citation {
            label: src.title.clone(),
            kind: src.kind.clone(),
        });
        let body: String = src.body.chars().take(12000).collect();
        parts.push(format!(
            "[{}] {} ({}):\n{body}",
            sources.len(),
            src.title,
            src.kind
        ));
    }
    known(&mut *conn, &s, &mut sources, &mut parts).await?;
    let edited: Vec<&str> = s
        .sections
        .iter()
        .filter(|x| !x.drafted && !x.body.is_empty())
        .map(|x| x.key.as_str())
        .collect();
    for x in s
        .sections
        .iter()
        .filter(|x| !x.drafted && !x.body.is_empty())
    {
        parts.push(format!(
            "Already written by people; keep consistent with it. {}:\n{}",
            x.title, x.body
        ));
    }
    let answered: Vec<String> = s
        .questions
        .iter()
        .filter_map(|q| {
            q.answer
                .as_ref()
                .map(|a| format!("Q: {}\nA: {a}", q.question))
        })
        .collect();
    if !answered.is_empty() {
        parts.push(format!(
            "Questions already answered:\n{}",
            answered.join("\n")
        ));
    }
    Ok(Context {
        text: parts.join("\n\n"),
        sections: t
            .sections
            .iter()
            .filter(|x| !edited.contains(&x.key))
            .map(|x| format!("{}: {}: {}", x.key, x.title, x.guidance))
            .collect(),
        sources,
    })
}

/// What Huxley knows that helps: the client's environment and agreements,
/// and approved scopes of past won work on the same template.
async fn known(
    conn: &mut PgConnection,
    s: &Scope,
    sources: &mut Vec<Citation>,
    parts: &mut Vec<String>,
) -> SalesResult<()> {
    let environment = environment(&mut *conn, s.org_id).await?;
    if !environment.is_empty() {
        sources.push(Citation {
            label: "The client's environment in Huxley".into(),
            kind: "environment".into(),
        });
        parts.push(format!(
            "[{}] The client's environment (from Huxley):\n{environment}",
            sources.len()
        ));
    }
    let agreements: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT name, kind, status FROM contract_agreements WHERE org_id = $1 AND status <> 'cancelled' ORDER BY name",
    )
    .bind(s.org_id)
    .fetch_all(&mut *conn)
    .await?;
    if !agreements.is_empty() {
        sources.push(Citation {
            label: "The client's agreements".into(),
            kind: "agreements".into(),
        });
        let list: Vec<String> = agreements
            .iter()
            .map(|(n, k, st)| format!("- {n} ({k}, {st})"))
            .collect();
        parts.push(format!(
            "[{}] The client's agreements:\n{}",
            sources.len(),
            list.join("\n")
        ));
    }
    let past: Vec<(String, Json)> = sqlx::query_as(
        "SELECT op.name, v.snapshot FROM sales_scope_versions v JOIN sales_scopes sc ON sc.id = v.scope_id \
           JOIN sales_opportunities op ON op.id = sc.opportunity_id \
          WHERE sc.template = $1 AND sc.id <> $2 AND op.status = 'won' \
            AND v.number = (SELECT max(number) FROM sales_scope_versions WHERE scope_id = sc.id) \
          ORDER BY v.created_at DESC LIMIT 2",
    )
    .bind(&s.template)
    .bind(s.id)
    .fetch_all(&mut *conn)
    .await?;
    for (name, snapshot) in past {
        sources.push(Citation {
            label: format!("Past scope: {name}"),
            kind: "past_work".into(),
        });
        let sections: Vec<String> = snapshot["sections"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| {
                        format!(
                            "## {}\n{}",
                            x["title"].as_str().unwrap_or(""),
                            x["body"].as_str().unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let joined: String = sections.join("\n").chars().take(6000).collect();
        parts.push(format!(
            "[{}] An approved scope of past won work, \"{name}\" (for style and structure only, not facts about this client):\n{joined}",
            sources.len()
        ));
    }
    Ok(())
}

/// The client's people, sites and devices, as lines of text.
pub(crate) async fn environment_text(conn: &mut PgConnection, org: Uuid) -> SalesResult<String> {
    environment(conn, org).await
}

async fn environment(conn: &mut PgConnection, org: Uuid) -> SalesResult<String> {
    let (contacts, sites): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM crm_contacts WHERE org_id = $1 AND archived_at IS NULL), \
                (SELECT count(*) FROM crm_sites WHERE org_id = $1 AND archived_at IS NULL)",
    )
    .bind(org)
    .fetch_one(&mut *conn)
    .await?;
    let devices: Vec<(String, i64)> = sqlx::query_as(
        "SELECT t.name, count(*) FROM assets a JOIN cmdb_asset_types t ON t.id = a.type_id \
          WHERE a.org_id = $1 GROUP BY t.name ORDER BY count(*) DESC LIMIT 12",
    )
    .bind(org)
    .fetch_all(&mut *conn)
    .await?;
    let mut lines = Vec::new();
    if contacts > 0 {
        lines.push(format!("- {contacts} contacts (people)"));
    }
    if sites > 0 {
        lines.push(format!("- {sites} sites"));
    }
    for (kind, n) in devices {
        lines.push(format!("- {n} × {kind}"));
    }
    Ok(lines.join("\n"))
}

/// What the AI wrote.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeDraft)]
pub struct Draft {
    #[serde(default)]
    pub sections: Vec<DraftSection>,
    #[serde(default)]
    pub questions: Vec<DraftQuestion>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeDraftSection)]
pub struct DraftSection {
    pub key: String,
    pub body: String,
    /// Source numbers it drew on.
    #[serde(default)]
    pub sources: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeDraftQuestion)]
pub struct DraftQuestion {
    #[serde(default)]
    pub section: Option<String>,
    pub question: String,
}

/// Keeps a draft from the workflow: sections people haven't edited get the
/// AI's text and citations; its open questions replace the AI's earlier
/// unresolved ones. Run in system scope.
pub async fn write_draft(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    draft: &Draft,
) -> SalesResult<usize> {
    let s = get(&mut *conn, id).await?;
    let ctx_sources = context(&mut *conn, id).await?.sources;
    let mut written = 0;
    for d in &draft.sections {
        let Some(sec) = s.sections.iter().find(|x| x.key == d.key) else {
            continue;
        };
        if !sec.drafted && !sec.body.is_empty() {
            continue;
        }
        let body: String = d.body.trim().chars().take(20000).collect();
        let citations: Vec<Citation> = d
            .sources
            .iter()
            .filter_map(|n| {
                usize::try_from(*n)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|i| ctx_sources.get(i))
            })
            .cloned()
            .collect();
        sqlx::query(
            "UPDATE sales_scope_sections SET body = $3, citations = $4, drafted = true, updated_by = NULL, \
                updated_at = now() WHERE scope_id = $1 AND key = $2",
        )
        .bind(id)
        .bind(&d.key)
        .bind(&body)
        .bind(serde_json::to_value(&citations).unwrap_or(Json::Array(Vec::new())))
        .execute(&mut *conn)
        .await?;
        written += 1;
    }
    sqlx::query("DELETE FROM sales_scope_questions WHERE scope_id = $1 AND from_ai AND resolved_at IS NULL AND answer IS NULL")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for q in draft.questions.iter().take(30) {
        let question: String = q.question.trim().chars().take(1000).collect();
        if question.is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO sales_scope_questions (id, scope_id, org_id, section_key, question, from_ai) \
             VALUES ($1, $2, $3, $4, $5, true)",
        )
        .bind(huxley_core::new_id())
        .bind(id)
        .bind(s.org_id)
        .bind(q.section.as_deref().filter(|k| s.sections.iter().any(|x| x.key == *k)))
        .bind(&question)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query("UPDATE sales_scopes SET draft_state = 'idle', drafted_at = now(), version = version + 1 WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.drafted")
            .org(s.org_id)
            .target("scope", id)
            .details(json!({ "sections": written, "questions": draft.questions.len() })),
    )
    .await?;
    Ok(written)
}

/// Records that drafting failed (the workflow's error branch).
pub async fn draft_failed(conn: &mut PgConnection, id: Uuid, error: &str) -> SalesResult<()> {
    sqlx::query("UPDATE sales_scopes SET draft_state = 'failed', draft_error = $2 WHERE id = $1")
        .bind(id)
        .bind(error.chars().take(2000).collect::<String>())
        .execute(conn)
        .await?;
    Ok(())
}

// ---- approving ----

/// Approves the Scope of Work as it stands: keeps it as the next version.
pub async fn approve(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    version: i32,
) -> SalesResult<Scope> {
    let s = get(&mut *conn, id).await?;
    if s.version != version {
        return Err(SalesError::VersionConflict);
    }
    if s.sections.iter().all(|x| x.body.trim().is_empty()) {
        return Err(SalesError::invalid("sections", "write the scope first"));
    }
    let number = s.versions.first().map_or(1, |v| v.number + 1);
    let snapshot = json!({
        "template": s.template,
        "sections": s.sections.iter().filter(|x| !x.body.trim().is_empty())
            .map(|x| json!({ "key": x.key, "title": x.title, "body": x.body })).collect::<Vec<_>>(),
    });
    let vid = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sales_scope_versions (id, scope_id, org_id, number, snapshot, created_by) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(vid)
    .bind(id)
    .bind(s.org_id)
    .bind(number)
    .bind(&snapshot)
    .bind(ctx.actor.id())
    .execute(&mut *conn)
    .await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("scope.approved")
            .org(s.org_id)
            .target("scope", id)
            .details(json!({ "version": number, "open_questions": s.questions.iter().filter(|q| q.resolved_at.is_none()).count() })),
    )
    .await?;
    outbox::emit(
        &mut *conn,
        DomainEvent {
            event_type: "scope.approved",
            org_id: Some(s.org_id),
            subject_type: "scope",
            subject_id: id,
            payload: json!({ "scopeId": id, "versionId": vid, "number": number, "opportunityId": s.opportunity_id }),
        },
    )
    .await?;
    get(conn, id).await
}

/// An approved version's sections (for a quote).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeOfWork)]
pub struct ScopeOfWork {
    pub version_id: Uuid,
    pub number: i32,
    #[schema(value_type = Vec<SowSection>)]
    pub sections: Vec<SowSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScopeOfWorkSection)]
pub struct SowSection {
    pub title: String,
    pub body: String,
}

/// An approved version, if it belongs to this opportunity.
pub async fn version_of(
    conn: &mut PgConnection,
    version: Uuid,
    opportunity: Option<Uuid>,
) -> SalesResult<ScopeOfWork> {
    let row: Option<(i32, Json, Uuid)> = sqlx::query_as(
        "SELECT v.number, v.snapshot, s.opportunity_id FROM sales_scope_versions v \
           JOIN sales_scopes s ON s.id = v.scope_id WHERE v.id = $1",
    )
    .bind(version)
    .fetch_optional(conn)
    .await?;
    let Some((number, snapshot, opp)) = row else {
        return Err(SalesError::invalid(
            "scopeVersionId",
            "not an approved scope",
        ));
    };
    if opportunity.is_some_and(|o| o != opp) {
        return Err(SalesError::invalid(
            "scopeVersionId",
            "that scope is another opportunity's",
        ));
    }
    let sections = snapshot["sections"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| SowSection {
                    title: x["title"].as_str().unwrap_or_default().to_owned(),
                    body: x["body"].as_str().unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ScopeOfWork {
        version_id: version,
        number,
        sections,
    })
}
