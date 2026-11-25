//! Global search (design doc §8.2): one query across tickets, clients,
//! contacts, workflows and people, for the command palette. Each kind is
//! searched only where its module is on, within the organizations where the
//! caller may read it; matches rank by trigram similarity.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(search))
}

/// Results per kind.
const PER_KIND: i64 = 5;

#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchParams {
    /// What to look for: at least 2 characters.
    pub q: String,
}

/// Something found.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// `ticket`, `client`, `contact`, `workflow` or `person`.
    pub kind: String,
    pub id: Uuid,
    pub title: String,
    /// A line of context: a ticket's reference and client, a contact's
    /// email and company, a person's email.
    pub subtitle: String,
}

fn db(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Whether a module is on.
async fn module_active(state: &AppState, key: &str) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    let on = huxley_modules::is_active(&mut tx, key).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(on)
}

/// Runs a query in `scope`.
async fn scoped<T>(
    state: &AppState,
    scope: &OrgScope,
    f: impl AsyncFnOnce(&mut PgConnection) -> Result<T, sqlx::Error>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, scope).await.map_err(db)?;
    let value = f(&mut tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(value)
}

/// `%…%` for ILIKE, with its wildcards escaped.
fn pattern(q: &str) -> String {
    format!(
        "%{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

async fn tickets(tx: &mut PgConnection, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    let number: Option<i64> = q.rsplit('-').next().and_then(|n| n.trim().parse().ok());
    sqlx::query_as(
        "SELECT 'ticket' AS kind, t.id, t.title, \
                t.number_prefix || '-' || t.number || ' · ' || o.name || ' · ' || s.name AS subtitle \
           FROM sd_tickets t JOIN organizations o ON o.id = t.org_id JOIN sd_statuses s ON s.id = t.status_id \
          WHERE t.title ILIKE $1 OR t.title % $2 OR t.number = $3 \
          ORDER BY t.number = $3 DESC NULLS LAST, s.category IN ('resolved', 'closed'), \
                   similarity(t.title, $2) DESC, t.created_at DESC \
          LIMIT $4",
    )
    .bind(pattern(q))
    .bind(q)
    .bind(number)
    .bind(PER_KIND)
    .fetch_all(tx)
    .await
}

async fn clients(tx: &mut PgConnection, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    sqlx::query_as(
        "SELECT 'client' AS kind, o.id, o.name AS title, coalesce(p.name, '') AS subtitle \
           FROM organizations o LEFT JOIN organizations p ON p.id = o.parent_id \
          WHERE o.kind = 'client' AND o.status = 'active' AND (o.name ILIKE $1 OR o.name % $2) \
          ORDER BY similarity(o.name, $2) DESC, o.name LIMIT $3",
    )
    .bind(pattern(q))
    .bind(q)
    .bind(PER_KIND)
    .fetch_all(tx)
    .await
}

async fn contacts(tx: &mut PgConnection, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    sqlx::query_as(
        "SELECT 'contact' AS kind, c.id, \
                coalesce(nullif(btrim(c.first_name || ' ' || c.last_name), ''), c.email, 'Contact') AS title, \
                concat_ws(' · ', c.email, o.name) AS subtitle \
           FROM crm_contacts c JOIN organizations o ON o.id = c.org_id \
          WHERE c.archived_at IS NULL \
            AND ((c.first_name || ' ' || c.last_name || ' ' || coalesce(c.email, '')) ILIKE $1 \
              OR (c.first_name || ' ' || c.last_name || ' ' || coalesce(c.email, '')) % $2) \
          ORDER BY similarity(c.first_name || ' ' || c.last_name || ' ' || coalesce(c.email, ''), $2) DESC \
          LIMIT $3",
    )
    .bind(pattern(q))
    .bind(q)
    .bind(PER_KIND)
    .fetch_all(tx)
    .await
}

async fn workflows(tx: &mut PgConnection, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    sqlx::query_as(
        "SELECT 'workflow' AS kind, w.id, w.name AS title, w.key || ' · ' || o.name AS subtitle \
           FROM workflows w JOIN organizations o ON o.id = w.org_id \
          WHERE w.status = 'active' AND (w.name ILIKE $1 OR w.key ILIKE $1 OR w.name % $2) \
          ORDER BY similarity(w.name, $2) DESC, w.name LIMIT $3",
    )
    .bind(pattern(q))
    .bind(q)
    .bind(PER_KIND)
    .fetch_all(tx)
    .await
}

async fn people(tx: &mut PgConnection, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    sqlx::query_as(
        "SELECT 'person' AS kind, u.id, u.display_name AS title, u.email AS subtitle \
           FROM users u \
          WHERE u.status = 'active' AND u.kind = 'human' \
            AND ((u.display_name || ' ' || u.email) ILIKE $1 OR (u.display_name || ' ' || u.email) % $2) \
          ORDER BY similarity(u.display_name || ' ' || u.email, $2) DESC LIMIT $3",
    )
    .bind(pattern(q))
    .bind(q)
    .bind(PER_KIND)
    .fetch_all(tx)
    .await
}

/// Searches what you can see: tickets, clients and contacts (where their
/// modules are on), workflows and people. Up to 5 of each, best first.
#[utoipa::path(get, path = "/api/v1/search", tag = "search", params(SearchParams),
    responses((status = 200, body = Vec<SearchHit>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn search(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<SearchParams>,
) -> ApiResult<Json<Vec<SearchHit>>> {
    let q = params.q.trim().to_owned();
    if q.chars().count() < 2 || q.chars().count() > 100 {
        return Err(ApiError::validation().with_field_error("/q", "2 to 100 characters"));
    }
    let granted = authed.principal.granted_anywhere();
    let mut hits = Vec::new();

    if module_active(&state, huxley_servicedesk::MODULE).await?
        && granted.contains(&permissions::TICKET_READ)
    {
        let scope = authed
            .module_scope(&state, permissions::TICKET_READ, huxley_servicedesk::MODULE)
            .await?;
        hits.extend(scoped(&state, &scope, async |tx| tickets(tx, &q).await).await?);
    }
    if module_active(&state, huxley_crm::MODULE).await? && granted.contains(&permissions::CRM_READ)
    {
        let scope = authed
            .module_scope(&state, permissions::CRM_READ, huxley_crm::MODULE)
            .await?;
        hits.extend(scoped(&state, &scope, async |tx| clients(tx, &q).await).await?);
        hits.extend(scoped(&state, &scope, async |tx| contacts(tx, &q).await).await?);
    }
    if granted.contains(&permissions::WORKFLOW_READ) {
        let scope = authed
            .permission_scope(&state, permissions::WORKFLOW_READ, None)
            .await?;
        hits.extend(scoped(&state, &scope, async |tx| workflows(tx, &q).await).await?);
    }
    let workforce = module_active(&state, huxley_workforce::MODULE).await?
        && granted.contains(&permissions::WORKFORCE_READ);
    if workforce || granted.contains(&permissions::USER_READ) {
        hits.extend(scoped(&state, &OrgScope::System, async |tx| people(tx, &q).await).await?);
    }
    Ok(Json(hits))
}
