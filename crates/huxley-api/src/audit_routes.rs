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
    Router::new()
        .route("/", get(list))
        .route("/export", get(export))
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

    // What's just been written is sealed into the chain before it's listed.
    let mut conn = state
        .db
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    huxley_db::audit::seal(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    drop(conn);
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    // Only the filters given: `($n IS NULL OR …)` filters leave prepared
    // statements' generic plans unable to use the indexes (docs/performance.md).
    let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "SELECT e.id, e.seq, e.occurred_at, e.actor_type, e.actor_id, u.display_name AS actor_name, \
                e.via, host(e.ip) AS ip, e.action, e.org_id, o.name AS org_name, \
                e.target_type, e.target_id, e.details \
           FROM audit_events e \
           LEFT JOIN users u ON u.id = e.actor_id \
           LEFT JOIN organizations o ON o.id = e.org_id \
          WHERE e.seq IS NOT NULL",
    );
    if let Some(before) = before {
        q.push(" AND e.seq < ").push_bind(before);
    }
    if let Some(prefix) = prefix {
        q.push(" AND e.action LIKE ")
            .push_bind(format!("{prefix}%"));
    }
    if let Some(target) = params.target_id {
        q.push(" AND e.target_id = ").push_bind(target);
    }
    q.push(" ORDER BY e.seq DESC LIMIT ")
        .push_bind(i64::from(limit) + 1);
    let rows: Vec<AuditEventView> = q
        .build_query_as()
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

#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// One JSON object per line.
    #[default]
    Jsonl,
    Csv,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ExportParams {
    #[serde(default)]
    #[param(inline)]
    pub format: ExportFormat,
    /// Only actions starting with this prefix.
    pub action: Option<String>,
    /// Events at or after this time (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub from: Option<OffsetDateTime>,
    /// Events before this time (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub to: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct ExportRow {
    seq: i64,
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
    actor_type: String,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    via: String,
    ip: Option<String>,
    user_agent: Option<String>,
    request_id: Option<String>,
    action: String,
    org_id: Option<Uuid>,
    org_name: Option<String>,
    target_type: Option<String>,
    target_id: Option<Uuid>,
    details: serde_json::Value,
    /// Hex; with `hash`, lets the export be checked against the chain.
    prev_hash: String,
    hash: String,
}

const CSV_HEADER: &str = "seq,id,occurred_at,actor_type,actor_id,actor_name,via,ip,user_agent,request_id,action,org_id,org_name,target_type,target_id,details,prev_hash,hash\n";

/// Quotes a CSV field, and defuses spreadsheet formulas (`=`, `+`, `-`, `@`).
fn csv_field(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn csv_line(r: &ExportRow) -> String {
    let opt = |v: Option<String>| v.unwrap_or_default();
    let fields = [
        r.seq.to_string(),
        r.id.to_string(),
        r.occurred_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        r.actor_type.clone(),
        opt(r.actor_id.map(|v| v.to_string())),
        opt(r.actor_name.clone()),
        r.via.clone(),
        opt(r.ip.clone()),
        opt(r.user_agent.clone()),
        opt(r.request_id.clone()),
        r.action.clone(),
        opt(r.org_id.map(|v| v.to_string())),
        opt(r.org_name.clone()),
        opt(r.target_type.clone()),
        opt(r.target_id.map(|v| v.to_string())),
        r.details.to_string(),
        r.prev_hash.clone(),
        r.hash.clone(),
    ];
    let mut line = fields
        .iter()
        .map(|f| csv_field(f))
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

const EXPORT_BATCH: i64 = 1000;

/// The next batch of export rows after `after`, in its own transaction.
async fn export_batch(
    db: &huxley_db::sqlx::PgPool,
    after: i64,
    prefix: Option<&str>,
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
) -> Result<Vec<ExportRow>, String> {
    let mut tx = begin_scoped(db, &OrgScope::System)
        .await
        .map_err(|e| e.to_string())?;
    let rows = sqlx::query_as(
        "SELECT e.seq, e.id, e.occurred_at, e.actor_type, e.actor_id, u.display_name AS actor_name, \
                e.via, host(e.ip) AS ip, e.user_agent, e.request_id, e.action, e.org_id, o.name AS org_name, \
                e.target_type, e.target_id, e.details, \
                encode(e.prev_hash, 'hex') AS prev_hash, encode(e.hash, 'hex') AS hash \
           FROM audit_events e \
           LEFT JOIN users u ON u.id = e.actor_id \
           LEFT JOIN organizations o ON o.id = e.org_id \
          WHERE e.seq > $1 \
            AND ($2::text IS NULL OR e.action LIKE $2 || '%') \
            AND ($3::timestamptz IS NULL OR e.occurred_at >= $3) \
            AND ($4::timestamptz IS NULL OR e.occurred_at < $4) \
          ORDER BY e.seq LIMIT $5",
    )
    .bind(after)
    .bind(prefix)
    .bind(from)
    .bind(to)
    .bind(EXPORT_BATCH)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Downloads the audit log, oldest first, as JSON lines or CSV. Streams in
/// batches, so large logs don't need to fit in memory. Each row carries its
/// chain hashes. The export itself is audited.
#[utoipa::path(get, path = "/api/v1/audit/export", operation_id = "export_audit_events", tag = "audit", params(ExportParams),
    responses((status = 200, description = "The events", content_type = "application/x-ndjson"),
        (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn export(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<ExportParams>,
) -> ApiResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse as _;
    use futures_util::StreamExt as _;
    authed.require_instance(permissions::AUDIT_READ)?;
    let prefix = params.action.as_deref().map(|a| {
        a.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    });
    // Record the export first: it shows up in itself, and a failed audit
    // write stops the export.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let format_name = match params.format {
        ExportFormat::Jsonl => "jsonl",
        ExportFormat::Csv => "csv",
    };
    huxley_db::audit::record(
        &mut tx,
        &authed.user.ctx,
        huxley_db::audit::AuditRecord::new("audit.exported").details(serde_json::json!({
            "format": format_name,
            "action": params.action,
            "from": params.from.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
            "to": params.to.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        })),
    )
    .await
    .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    // Everything committed so far, the export's own record included, is
    // sealed into the chain before it's exported (ADR-086).
    let mut conn = state
        .db
        .acquire()
        .await
        .map_err(|e| ApiError::internal(&e))?;
    huxley_db::audit::seal(&mut conn)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    drop(conn);

    let (db, format, from, to) = (state.db.clone(), params.format, params.from, params.to);
    let header_chunk = matches!(format, ExportFormat::Csv)
        .then(|| Ok::<_, std::io::Error>(bytes::Bytes::from_static(CSV_HEADER.as_bytes())));
    let batches = futures_util::stream::unfold(Some(0_i64), move |after| {
        let (db, prefix) = (db.clone(), prefix.clone());
        async move {
            let after = after?;
            let rows = export_batch(&db, after, prefix.as_deref(), from, to).await;
            match rows {
                Err(e) => {
                    tracing::error!(error = %e, "audit export failed mid-stream");
                    Some((Err(std::io::Error::other("audit export failed")), None))
                }
                Ok(rows) if rows.is_empty() => None,
                Ok(rows) => {
                    let next = (i64::try_from(rows.len()).unwrap_or(i64::MAX) == EXPORT_BATCH)
                        .then(|| rows.last().map_or(after, |r| r.seq));
                    let mut chunk = String::new();
                    for r in &rows {
                        match format {
                            ExportFormat::Jsonl => {
                                chunk.push_str(&serde_json::to_string(r).unwrap_or_default());
                                chunk.push('\n');
                            }
                            ExportFormat::Csv => chunk.push_str(&csv_line(r)),
                        }
                    }
                    Some((Ok(bytes::Bytes::from(chunk)), next))
                }
            }
        }
    });
    let body =
        axum::body::Body::from_stream(futures_util::stream::iter(header_chunk).chain(batches));
    let stamp = OffsetDateTime::now_utc().unix_timestamp();
    let (content_type, ext) = match format {
        ExportFormat::Jsonl => ("application/x-ndjson", "jsonl"),
        ExportFormat::Csv => ("text/csv; charset=utf-8", "csv"),
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"huxley-audit-{stamp}.{ext}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_fields_are_quoted_and_formulas_defused() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(csv_field("-1+2,3"), "\"'-1+2,3\"");
    }
}
