//! `/api/v1/audit`: the audit log, newest first.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_core::{Cursor, PageRequest};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(list))
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventView {
    pub id: Uuid,
    pub seq: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// `user`, `system`, or `anonymous`.
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    /// `ui`, `api`, `cli`, `mcp`, `workflow`, or `system`.
    pub via: String,
    pub ip: Option<String>,
    pub action: String,
    pub org_id: Option<Uuid>,
    pub org_name: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    #[schema(value_type = Object)]
    pub details: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    pub items: Vec<AuditEventView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AuditParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Only actions starting with this prefix, e.g. `auth.` or `org.created`.
    pub action: Option<String>,
    /// Only events about this entity.
    pub target_id: Option<Uuid>,
}

/// Lists audit events, newest first.
#[utoipa::path(get, path = "/api/v1/audit", operation_id = "list_audit_events", tag = "audit", params(AuditParams),
    responses((status = 200, body = AuditPage), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<AuditParams>,
) -> ApiResult<Json<AuditPage>> {
    authed.require_instance(permissions::AUDIT_READ)?;
    let page = PageRequest {
        limit: params.limit,
        cursor: params.cursor.map(Cursor::from),
    };
    let before: Option<i64> = page.cursor.as_ref().map(Cursor::decode).transpose()?;
    let limit = page.limit();
    // Escape LIKE wildcards so the prefix is matched literally.
    let prefix = params.action.map(|a| {
        a.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    });

    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let rows: Vec<AuditEventView> = sqlx::query_as(
        "SELECT e.id, e.seq, e.occurred_at, e.actor_type, e.actor_id, u.display_name AS actor_name, \
                e.via, host(e.ip) AS ip, e.action, e.org_id, o.name AS org_name, \
                e.target_type, e.target_id, e.details \
           FROM audit_events e \
           LEFT JOIN users u ON u.id = e.actor_id \
           LEFT JOIN organizations o ON o.id = e.org_id \
          WHERE ($1::bigint IS NULL OR e.seq < $1) \
            AND ($2::text IS NULL OR e.action LIKE $2 || '%') \
            AND ($3::uuid IS NULL OR e.target_id = $3) \
          ORDER BY e.seq DESC LIMIT $4",
    )
    .bind(before)
    .bind(prefix)
    .bind(params.target_id)
    .bind(i64::from(limit) + 1)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;

    let page = huxley_core::Page::from_overfetch(rows, limit, |e| e.seq);
    Ok(Json(AuditPage {
        items: page.items,
        next_cursor: page.next_cursor.map(|c| c.as_str().to_owned()),
    }))
}
