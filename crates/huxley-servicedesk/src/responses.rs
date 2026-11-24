//! Canned responses (design doc §8.2): shared replies and internal notes.
//! Their placeholders (`{{requester.first_name}}` and the like) are filled
//! in for a ticket and the technician using them; unknown ones are left as
//! written. Instance-wide.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::tickets::Ticket;
use crate::{DeskError, DeskResult, text};

/// The placeholders a response can use.
pub const PLACEHOLDERS: &[(&str, &str)] = &[
    ("requester.first_name", "The requester's first name"),
    ("requester.name", "The requester's full name"),
    ("ticket.reference", "The ticket's reference, e.g. T-1042"),
    ("ticket.title", "The ticket's title"),
    ("client.name", "The ticket's organization"),
    ("me.name", "Your name"),
    ("me.first_name", "Your first name"),
];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CannedResponse {
    pub id: Uuid,
    pub name: String,
    pub category: String,
    /// `public` (a reply to the requester) or `internal`.
    pub kind: String,
    pub body: String,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CannedResponseInput {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    /// `public` (the default) or `internal`.
    #[serde(default)]
    pub kind: Option<String>,
    pub body: String,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

const SELECT: &str = "SELECT id, name, category, kind, body, version FROM sd_canned_responses WHERE archived_at IS NULL";

pub async fn list(conn: &mut PgConnection) -> DeskResult<Vec<CannedResponse>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY lower(category), lower(name)"
    )))
    .fetch_all(conn)
    .await?)
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> DeskResult<CannedResponse> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND id = $1")))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(DeskError::NotFound)
}

struct Checked {
    name: String,
    category: String,
    kind: String,
    body: String,
}

fn check(input: &CannedResponseInput) -> DeskResult<Checked> {
    let name = text("name", Some(&input.name), 100)?
        .ok_or_else(|| DeskError::invalid("name", "a response needs a name"))?;
    let category = text("category", input.category.as_deref(), 100)?.unwrap_or_default();
    let kind = input.kind.clone().unwrap_or_else(|| "public".into());
    if kind != "public" && kind != "internal" {
        return Err(DeskError::invalid("kind", "public or internal"));
    }
    let body = text("body", Some(&input.body), 20_000)?
        .ok_or_else(|| DeskError::invalid("body", "a response needs some text"))?;
    Ok(Checked {
        name,
        category,
        kind,
        body,
    })
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_canned_responses_name") => {
            DeskError::invalid("name", "another response has this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    input: &CannedResponseInput,
) -> DeskResult<CannedResponse> {
    let c = check(input)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_canned_responses (id, name, category, kind, body) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(&c.name)
    .bind(&c.category)
    .bind(&c.kind)
    .bind(&c.body)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("canned_response.created")
            .target("canned_response", id)
            .details(json!({ "name": c.name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    id: Uuid,
    input: &CannedResponseInput,
) -> DeskResult<CannedResponse> {
    let c = check(input)?;
    let rows = sqlx::query(
        "UPDATE sd_canned_responses SET name = $3, category = $4, kind = $5, body = $6, version = version + 1 \
          WHERE id = $1 AND version = $2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&c.name)
    .bind(&c.category)
    .bind(&c.kind)
    .bind(&c.body)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        get(conn, id).await?;
        return Err(DeskError::VersionConflict);
    }
    audit::record(
        conn,
        ctx,
        AuditRecord::new("canned_response.updated")
            .target("canned_response", id)
            .details(json!({ "name": c.name })),
    )
    .await?;
    get(conn, id).await
}

pub async fn archive(conn: &mut PgConnection, ctx: &RequestContext, id: Uuid) -> DeskResult<()> {
    let found = get(conn, id).await?;
    sqlx::query("UPDATE sd_canned_responses SET archived_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    audit::record(
        conn,
        ctx,
        AuditRecord::new("canned_response.archived")
            .target("canned_response", id)
            .details(json!({ "name": found.name })),
    )
    .await?;
    Ok(())
}

/// Fills in `template`'s placeholders from `values` (`{{ key }}`, spaces
/// optional). Unknown placeholders stay as written.
pub fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = after[..end].trim();
        match values.iter().find(|(k, _)| *k == key) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn first_word(name: &str) -> String {
    name.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Every response, filled in for `ticket` and the technician `me`.
pub async fn for_ticket(
    conn: &mut PgConnection,
    ticket: &Ticket,
    me: &str,
) -> DeskResult<Vec<CannedResponse>> {
    let first: Option<String> = match ticket.requester_id {
        Some(id) => sqlx::query_scalar(
            "SELECT nullif(btrim(first_name), '') FROM crm_contacts WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .flatten(),
        None => None,
    };
    let requester = ticket.requester_name.clone().unwrap_or_default();
    let values = [
        (
            "requester.first_name",
            first.unwrap_or_else(|| first_word(&requester)),
        ),
        ("requester.name", requester),
        ("ticket.reference", ticket.reference.clone()),
        ("ticket.title", ticket.title.clone()),
        ("client.name", ticket.org_name.clone()),
        ("me.name", me.to_owned()),
        ("me.first_name", first_word(me)),
    ];
    Ok(list(conn)
        .await?
        .into_iter()
        .map(|mut r| {
            r.body = fill(&r.body, &values);
            r
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_and_unknown_ones_kept() {
        let values = [
            ("requester.first_name", "Ada".to_owned()),
            ("ticket.reference", "T-7".to_owned()),
        ];
        assert_eq!(
            fill(
                "Hi {{requester.first_name}}, about {{ ticket.reference }}: {{nope}} {{",
                &values
            ),
            "Hi Ada, about T-7: {{nope}} {{"
        );
        assert_eq!(fill("no placeholders", &values), "no placeholders");
        assert_eq!(
            fill("{{ticket.reference}}{{ticket.reference}}", &values),
            "T-7T-7"
        );
    }
}
