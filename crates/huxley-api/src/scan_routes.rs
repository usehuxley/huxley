//! Malware scanning of uploads, for administrators (ADR-094): how scanning
//! is set up, and the files it quarantined or couldn't check, to scan
//! again or delete.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::scans::{self, FileScan};
use huxley_db::{OrgScope, begin_scoped};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::authz::Authed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/platform/file-scans", get(list))
        .route("/platform/file-scans/status", get(status))
        .route("/platform/file-scans/{id}/rescan", post(rescan))
        .route("/platform/file-scans/{id}", delete(remove))
}

fn db(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// How uploads are scanned.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// `off`, `clamav` or `metadefender`.
    pub provider: String,
    /// Files not scanned clean yet are withheld (`uploads.scan.while_pending = "hold"`).
    pub holds_unscanned: bool,
    /// Whether the scanner answered just now (absent with scanning off).
    pub reachable: Option<bool>,
    /// The scanner's version, or why it didn't answer.
    pub detail: Option<String>,
    pub pending: i64,
    pub clean: i64,
    pub infected: i64,
    pub failed: i64,
}

#[utoipa::path(get, path = "/api/v1/platform/file-scans/status", operation_id = "file_scan_status", tag = "security",
    responses((status = 200, body = ScanStatus)))]
pub async fn status(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<ScanStatus>> {
    authed.require_instance(permissions::FILE_SCAN_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    let counts = scans::counts(&mut tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    let count = |s: &str| counts.iter().find(|(k, _)| k == s).map_or(0, |(_, n)| *n);
    let (provider, reachable, detail) = match &state.scanner {
        None => ("off".to_owned(), None, None),
        Some(scanner) => {
            let check =
                tokio::time::timeout(std::time::Duration::from_secs(10), scanner.check()).await;
            let (ok, detail) = match check {
                Ok(Ok(version)) => (true, version),
                Ok(Err(e)) => (false, e.0),
                Err(_) => (false, "didn't answer within 10 seconds".to_owned()),
            };
            (scanner.name().to_owned(), Some(ok), Some(detail))
        }
    };
    Ok(Json(ScanStatus {
        provider,
        holds_unscanned: state.storage.holds_unscanned(),
        reachable,
        detail,
        pending: count("pending"),
        clean: count("clean"),
        infected: count("infected"),
        failed: count("failed"),
    }))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `infected` (the default), `failed` or `pending`.
    pub status: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/platform/file-scans", operation_id = "list_file_scans", tag = "security",
    params(ListQuery), responses((status = 200, body = Vec<FileScan>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: Authed,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<FileScan>>> {
    authed.require_instance(permissions::FILE_SCAN_MANAGE)?;
    let status = query.status.as_deref().unwrap_or("infected");
    if !matches!(status, "infected" | "failed" | "pending") {
        return Err(
            ApiError::validation().with_field_error("/status", "infected, failed or pending")
        );
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    let files = scans::list(&mut tx, status, 200).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(Json(files))
}

async fn found(state: &AppState, id: Uuid) -> ApiResult<FileScan> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    let scan = scans::get(&mut tx, id).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    scan.ok_or_else(ApiError::not_found)
}

#[utoipa::path(post, path = "/api/v1/platform/file-scans/{id}/rescan", operation_id = "rescan_file", tag = "security",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn rescan(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::FILE_SCAN_MANAGE)?;
    if state.scanner.is_none() {
        return Err(ApiError::conflict(
            "Scanning is off, so nothing would scan it.",
        ));
    }
    let scan = found(&state, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    scans::rescan(&mut tx, id).await.map_err(db)?;
    huxley_db::audit::record(
        &mut tx,
        &authed.user.ctx,
        huxley_db::audit::AuditRecord::new("file.rescan_requested")
            .org(scan.org_id)
            .target(&scan.source, scan.record_id)
            .details(serde_json::json!({ "filename": scan.filename, "was": scan.status })),
    )
    .await
    .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes the file itself: the attachment, or the expense entry's receipt
/// (the entry stays).
#[utoipa::path(delete, path = "/api/v1/platform/file-scans/{id}", operation_id = "delete_scanned_file", tag = "security",
    params(("id" = Uuid, Path)), responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::FILE_SCAN_MANAGE)?;
    let scan = found(&state, id).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db)?;
    let ctx = &authed.user.ctx;
    match scan.source.as_str() {
        "attachment" => {
            match huxley_entity::attachments::delete(&mut tx, &state.storage, ctx, scan.record_id)
                .await
            {
                Ok(()) | Err(huxley_entity::EntityError::NotFound) => {}
                Err(e) => return Err(crate::entity_routes::map_entity(e)),
            }
        }
        _ => match huxley_expenses::entries::drop_receipt(
            &mut tx,
            &state.storage,
            ctx,
            scan.record_id,
        )
        .await
        {
            Ok(()) | Err(huxley_expenses::ExpenseError::NotFound) => {}
            Err(e) => return Err(db(e)),
        },
    }
    scans::forget(&mut tx, id).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}
