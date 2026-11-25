//! Saved ticket views (design doc §8.2): a named set of filters on the
//! Tickets list. Someone's own, or shared with everyone (which only
//! service desk configurers manage). A view holds filters, not tickets:
//! what it shows still depends on who's looking.

use huxley_core::RequestContext;
use huxley_db::audit::{self, AuditRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{DeskError, DeskResult, text};

/// The Tickets list's built-in views a saved one starts from.
pub const BASES: &[&str] = &["mine", "unassigned", "open", "resolved", "closed"];

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TicketView {
    pub id: Uuid,
    pub name: String,
    /// Shared with everyone (else it's yours).
    pub shared: bool,
    /// `view` (a built-in view: `mine`, `unassigned`, `open`, `resolved`
    /// or `closed`), `queueId`, `orgId`, `search` and `priority` (e.g.
    /// `p1,p2`); each optional.
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub filters: Json,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TicketViewInput {
    pub name: String,
    #[schema(value_type = std::collections::HashMap<String, String>)]
    pub filters: Json,
    /// Shared with everyone (needs `servicedesk:configure`). Fixed once
    /// made.
    #[serde(default)]
    pub shared: bool,
    /// The version you read (updates only).
    #[serde(default)]
    pub version: Option<i32>,
}

/// The filters, checked: known keys, string values, real ids and values.
pub fn check_filters(filters: &Json) -> DeskResult<Json> {
    let Some(given) = filters.as_object() else {
        return Err(DeskError::invalid("filters", "an object of filters"));
    };
    let mut out = Map::new();
    for (key, value) in given {
        let Some(value) = value.as_str().map(str::trim) else {
            if value.is_null() {
                continue;
            }
            return Err(DeskError::invalid("filters", format!("{key} is text")));
        };
        if value.is_empty() {
            continue;
        }
        let ok = match key.as_str() {
            "view" => BASES.contains(&value),
            "queueId" | "orgId" => value.parse::<Uuid>().is_ok(),
            "search" => value.chars().count() <= 200,
            "priority" => value
                .split(',')
                .all(|p| crate::tickets::PRIORITIES.contains(&p.trim())),
            _ => {
                return Err(DeskError::invalid(
                    "filters",
                    format!("{key:?} isn't a filter"),
                ));
            }
        };
        if !ok {
            return Err(DeskError::invalid(
                "filters",
                format!("{key}: {value:?} isn't valid"),
            ));
        }
        out.insert(key.clone(), Json::String(value.to_owned()));
    }
    Ok(Json::Object(out))
}

const SELECT: &str =
    "SELECT id, name, owner_id IS NULL AS shared, filters, version FROM sd_ticket_views";

/// `user`'s views and the shared ones: shared first, then by name.
pub async fn list(conn: &mut PgConnection, user: Uuid) -> DeskResult<Vec<TicketView>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE owner_id IS NULL OR owner_id = $1 ORDER BY owner_id IS NOT NULL, lower(name)"
    )))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// A view and its owner (none: shared), if `user` can see it.
pub async fn get(
    conn: &mut PgConnection,
    user: Uuid,
    id: Uuid,
) -> DeskResult<(TicketView, Option<Uuid>)> {
    let view: TicketView = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE id = $1 AND (owner_id IS NULL OR owner_id = $2)"
    )))
    .bind(id)
    .bind(user)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(DeskError::NotFound)?;
    let owner = (!view.shared).then_some(user);
    Ok((view, owner))
}

fn taken(e: sqlx::Error) -> DeskError {
    match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("sd_ticket_views_name") => {
            DeskError::invalid("name", "there's already a view with this name")
        }
        _ => DeskError::Sqlx(e),
    }
}

fn name(input: &TicketViewInput) -> DeskResult<String> {
    text("name", Some(&input.name), 60)?
        .ok_or_else(|| DeskError::invalid("name", "a view needs a name"))
}

/// Saves a view: `owner` none makes it shared.
pub async fn create(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    owner: Option<Uuid>,
    input: &TicketViewInput,
) -> DeskResult<TicketView> {
    let name = name(input)?;
    let filters = check_filters(&input.filters)?;
    let id = huxley_core::new_id();
    sqlx::query(
        "INSERT INTO sd_ticket_views (id, owner_id, name, filters) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(owner)
    .bind(&name)
    .bind(&filters)
    .execute(&mut *conn)
    .await
    .map_err(taken)?;
    if owner.is_none() {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("ticket_view.created")
                .target("ticket_view", id)
                .details(json!({ "name": name, "filters": filters })),
        )
        .await?;
    }
    let (view, _) = get(conn, owner.unwrap_or_default(), id).await?;
    Ok(view)
}

/// Renames a view or changes its filters.
pub async fn update(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    id: Uuid,
    input: &TicketViewInput,
) -> DeskResult<TicketView> {
    let name = name(input)?;
    let filters = check_filters(&input.filters)?;
    let (before, _) = get(conn, user, id).await?;
    let rows = sqlx::query(
        "UPDATE sd_ticket_views SET name = $3, filters = $4, version = version + 1 WHERE id = $1 AND version = $2",
    )
    .bind(id)
    .bind(input.version.unwrap_or(-1))
    .bind(&name)
    .bind(&filters)
    .execute(&mut *conn)
    .await
    .map_err(taken)?
    .rows_affected();
    if rows == 0 {
        return Err(DeskError::VersionConflict);
    }
    if before.shared {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("ticket_view.updated")
                .target("ticket_view", id)
                .details(json!({ "name": name, "filters": filters })),
        )
        .await?;
    }
    Ok(get(conn, user, id).await?.0)
}

pub async fn delete(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    user: Uuid,
    id: Uuid,
) -> DeskResult<()> {
    let (view, _) = get(conn, user, id).await?;
    sqlx::query("DELETE FROM sd_ticket_views WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    if view.shared {
        audit::record(
            conn,
            ctx,
            AuditRecord::new("ticket_view.deleted")
                .target("ticket_view", id)
                .details(json!({ "name": view.name })),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_are_checked_and_trimmed() {
        let q = Uuid::now_v7().to_string();
        assert_eq!(
            check_filters(&json!({ "view": "open", "queueId": q, "priority": "p1,p2", "search": " vpn ", "orgId": "" }))
                .unwrap(),
            json!({ "view": "open", "queueId": q, "priority": "p1,p2", "search": "vpn" })
        );
        for bad in [
            json!({ "view": "everything" }),
            json!({ "queueId": "nope" }),
            json!({ "priority": "p1,p9" }),
            json!({ "colour": "red" }),
            json!({ "search": 3 }),
            json!(["open"]),
        ] {
            assert!(check_filters(&bad).is_err(), "{bad}");
        }
    }
}
