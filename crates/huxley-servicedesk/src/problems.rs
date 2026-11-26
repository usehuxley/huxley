//! Problem management (ADR-055): a problem ticket's root cause, workaround
//! and known-error status, and the incidents it causes (ticket links).

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult};

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProblemRecord)]
pub struct Problem {
    pub ticket_id: Uuid,
    pub org_id: Uuid,
    pub root_cause: String,
    pub workaround: String,
    /// The cause is known and there's a workaround.
    pub known_error: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub known_error_at: Option<OffsetDateTime>,
    pub kb_article_id: Option<Uuid>,
    /// Incidents linked to it.
    pub incidents: i64,
    pub version: i32,
}

const SELECT: &str = "SELECT p.ticket_id, p.org_id, p.root_cause, p.workaround, p.known_error, p.known_error_at, \
     p.kb_article_id, (SELECT count(*) FROM sd_ticket_links l WHERE l.ticket_id = p.ticket_id AND l.kind = 'incident') \
     AS incidents, p.version FROM sd_problems p";

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ProblemInput)]
pub struct Input {
    #[serde(default)]
    pub root_cause: Option<String>,
    #[serde(default)]
    pub workaround: Option<String>,
    #[serde(default)]
    pub known_error: Option<bool>,
    #[serde(default)]
    pub version: Option<i32>,
}

async fn problem_ticket(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Uuid> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT t.org_id, ty.key FROM sd_tickets t JOIN sd_ticket_types ty ON ty.id = t.type_id WHERE t.id = $1",
    )
    .bind(ticket)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        None => Err(DeskError::NotFound),
        Some((_, kind)) if kind != "problem" => {
            Err(DeskError::invalid("ticket", "not a problem ticket"))
        }
        Some((org, _)) => Ok(org),
    }
}

/// A problem ticket's details (empty at first).
pub async fn get(conn: &mut PgConnection, ticket: Uuid) -> DeskResult<Problem> {
    let org = problem_ticket(&mut *conn, ticket).await?;
    sqlx::query(
        "INSERT INTO sd_problems (ticket_id, org_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(ticket)
    .bind(org)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE p.ticket_id = $1"
    )))
    .bind(ticket)
    .fetch_one(conn)
    .await?)
}

/// Changes a problem. Marking it a known error needs a root cause and a
/// workaround; doing so raises `problem.known_error`.
pub async fn save(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    ticket: Uuid,
    input: &Input,
) -> DeskResult<Problem> {
    let current = get(&mut *conn, ticket).await?;
    if input.version.is_some_and(|v| v != current.version) {
        return Err(DeskError::VersionConflict);
    }
    let text = |v: Option<&String>, cur: &str, field: &str| -> DeskResult<String> {
        let v = v.map_or_else(|| cur.to_owned(), |s| s.trim().to_owned());
        if v.chars().count() > 20_000 {
            return Err(DeskError::invalid(field, "at most 20,000 characters"));
        }
        Ok(v)
    };
    let root_cause = text(input.root_cause.as_ref(), &current.root_cause, "rootCause")?;
    let workaround = text(input.workaround.as_ref(), &current.workaround, "workaround")?;
    let known = input.known_error.unwrap_or(current.known_error);
    if known && (root_cause.is_empty() || workaround.is_empty()) {
        return Err(DeskError::invalid(
            "knownError",
            "a known error needs a root cause and a workaround",
        ));
    }
    sqlx::query(
        "UPDATE sd_problems SET root_cause = $2, workaround = $3, known_error = $4, \
                known_error_at = CASE WHEN $4 AND NOT known_error THEN now() WHEN NOT $4 THEN NULL ELSE known_error_at END, \
                version = version + 1 WHERE ticket_id = $1",
    )
    .bind(ticket)
    .bind(&root_cause)
    .bind(&workaround)
    .bind(known)
    .execute(&mut *conn)
    .await?;
    let saved = get(&mut *conn, ticket).await?;
    audit::record(
        &mut *conn,
        ctx,
        AuditRecord::new("problem.updated")
            .org(saved.org_id)
            .target("ticket", ticket),
    )
    .await?;
    if known && !current.known_error {
        let (reference, title): (String, String) = sqlx::query_as(
            "SELECT number_prefix || '-' || number, title FROM sd_tickets WHERE id = $1",
        )
        .bind(ticket)
        .fetch_one(&mut *conn)
        .await?;
        outbox::emit(
            conn,
            DomainEvent {
                event_type: "problem.known_error",
                org_id: Some(saved.org_id),
                subject_type: "ticket",
                subject_id: ticket,
                payload: json!({
                    "reference": reference, "title": title, "rootCause": saved.root_cause,
                    "workaround": saved.workaround, "incidents": saved.incidents,
                }),
            },
        )
        .await?;
    }
    Ok(saved)
}

/// Notes the knowledge article drafted from the workaround.
pub async fn set_article(conn: &mut PgConnection, ticket: Uuid, article: Uuid) -> DeskResult<()> {
    sqlx::query("UPDATE sd_problems SET kb_article_id = $2 WHERE ticket_id = $1")
        .bind(ticket)
        .bind(article)
        .execute(conn)
        .await?;
    Ok(())
}
