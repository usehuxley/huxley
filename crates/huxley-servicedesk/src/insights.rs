//! Ticket insights (ADR-050): an AI summary with the entities a ticket
//! mentions and a next step (written by the AI ticket insights pack), and
//! similar tickets found from the ticket's own embedding.

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::PgConnection;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::embeddings::MeaningHit;
use crate::{DeskError, DeskResult};

/// The kinds of entity kept, each a list of short names.
pub const ENTITY_KINDS: &[&str] = &["people", "devices", "applications", "errors"];

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
#[schema(as = TicketInsight)]
pub struct Insight {
    pub ticket_id: Uuid,
    pub summary: String,
    /// `{ people, devices, applications, errors }`, each a list of names.
    #[schema(value_type = Object)]
    pub entities: Json,
    pub next_step: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Keeps known kinds, each up to 10 short distinct strings.
fn clean_entities(given: &Json) -> Json {
    let mut out = serde_json::Map::new();
    for kind in ENTITY_KINDS {
        let mut names: Vec<String> = Vec::new();
        for v in given[*kind].as_array().into_iter().flatten() {
            if let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                let s: String = s.chars().take(80).collect();
                if !names.iter().any(|n| n.eq_ignore_ascii_case(&s)) && names.len() < 10 {
                    names.push(s);
                }
            }
        }
        out.insert((*kind).to_owned(), json!(names));
    }
    Json::Object(out)
}

/// Records a ticket's insight (again replaces it).
pub async fn record(
    conn: &mut PgConnection,
    ticket_id: Uuid,
    summary: &str,
    entities: &Json,
    next_step: Option<&str>,
) -> DeskResult<Insight> {
    let summary: String = summary.trim().chars().take(1000).collect();
    if summary.is_empty() {
        return Err(DeskError::invalid(
            "summary",
            "say what the ticket is about",
        ));
    }
    let next_step: Option<String> = next_step
        .map(|s| s.trim().chars().take(300).collect())
        .filter(|s: &String| !s.is_empty());
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM sd_tickets WHERE id = $1")
        .bind(ticket_id)
        .fetch_optional(&mut *conn)
        .await?;
    let org = org.ok_or(DeskError::NotFound)?;
    Ok(sqlx::query_as(
        "INSERT INTO sd_ticket_insights (ticket_id, org_id, summary, entities, next_step) VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (ticket_id) DO UPDATE SET summary = EXCLUDED.summary, entities = EXCLUDED.entities, \
             next_step = EXCLUDED.next_step, at = now() \
         RETURNING ticket_id, summary, entities, next_step, at",
    )
    .bind(ticket_id)
    .bind(org)
    .bind(&summary)
    .bind(clean_entities(entities))
    .bind(&next_step)
    .fetch_one(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, ticket_id: Uuid) -> DeskResult<Option<Insight>> {
    Ok(sqlx::query_as("SELECT ticket_id, summary, entities, next_step, at FROM sd_ticket_insights WHERE ticket_id = $1")
        .bind(ticket_id)
        .fetch_optional(conn)
        .await?)
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SimilarTickets)]
pub struct Similar {
    /// Whether similar tickets can be found (pgvector, and the ticket is
    /// embedded).
    pub available: bool,
    /// Why not, when not.
    pub message: Option<String>,
    pub tickets: Vec<MeaningHit>,
}

/// Tickets nearest in meaning to `ticket_id` (by its own embedding, so no
/// AI call), in the transaction's scope, best first.
pub async fn similar(conn: &mut PgConnection, ticket_id: Uuid, limit: i64) -> DeskResult<Similar> {
    if !crate::embeddings::available(&mut *conn).await? {
        return Ok(Similar {
            available: false,
            message: Some("Finding similar tickets needs pgvector in the database.".into()),
            tickets: Vec::new(),
        });
    }
    let own: Option<(String, Vec<f32>)> =
        sqlx::query_as("SELECT model, embedding FROM sd_ticket_embeddings WHERE ticket_id = $1")
            .bind(ticket_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((model, vector)) = own else {
        return Ok(Similar {
            available: false,
            message: Some(
                "This ticket isn't embedded yet (the Service desk search by meaning pack embeds tickets every 5 minutes)."
                    .into(),
            ),
            tickets: Vec::new(),
        });
    };
    let hits = crate::embeddings::search(&mut *conn, &model, &vector, limit + 1).await?;
    Ok(Similar {
        available: true,
        message: None,
        tickets: hits
            .into_iter()
            .filter(|h| h.id != ticket_id)
            .take(usize::try_from(limit).unwrap_or(5))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_keep_known_kinds_short_and_distinct() {
        let cleaned = clean_entities(&json!({
            "people": ["Ada", "ada", " ", "Grace"],
            "devices": ["LAPTOP-42"],
            "secrets": ["hunter2"],
        }));
        assert_eq!(
            cleaned,
            json!({ "people": ["Ada", "Grace"], "devices": ["LAPTOP-42"], "applications": [], "errors": [] })
        );
    }
}
