//! Invoices in the client portal (ADR-078): the company's invoices and
//! credit memos that have gone out, with what's owed (from the accounting
//! system) and the link to pay; their PDFs. For approvers and admins.

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use huxley_billing::status::{self, PortalInvoice};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use uuid::Uuid;

use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/invoices", get(list))
        .route("/invoices/{id}/pdf", get(pdf))
}

/// Whether invoicing is on for the company.
async fn on(state: &AppState, org: Uuid) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let on = huxley_modules::is_active_at(&mut tx, huxley_billing::MODULE, org)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(on)
}

/// The company's invoices, newest first. Empty when invoicing is off.
#[utoipa::path(get, path = "/api/portal/v1/invoices", operation_id = "portal_invoices", tag = "portal",
    responses((status = 200, body = Vec<PortalInvoice>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<PortalInvoice>>> {
    let org = authed.principal.org_id;
    if !authed.principal.can(Perm::Invoices) {
        return Err(ApiError::forbidden());
    }
    if !on(&state, org).await? {
        return Ok(Json(Vec::new()));
    }
    let mut tx = begin_scoped(&state.db, &authed.scope())
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let list = status::portal_list(&mut tx, org)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(list))
}

/// One of the company's invoices, as a PDF.
#[utoipa::path(get, path = "/api/portal/v1/invoices/{id}/pdf", operation_id = "portal_invoice_pdf", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, content_type = "application/pdf")))]
pub async fn pdf(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let org = authed.principal.org_id;
    if !authed.principal.can(Perm::Invoices) || !on(&state, org).await? {
        return Err(ApiError::not_found());
    }
    let mut tx = begin_scoped(&state.db, &authed.scope())
        .await
        .map_err(|e| ApiError::internal(&e))?;
    if !status::portal_owns(&mut tx, org, id)
        .await
        .map_err(|e| ApiError::internal(&e))?
    {
        return Err(ApiError::not_found());
    }
    let i = huxley_billing::invoices::get(&mut tx, id)
        .await
        .map_err(|_| ApiError::not_found())?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"INV-{}.pdf\"", i.number),
            ),
        ],
        crate::billing_routes::render_pdf(&i),
    ))
}
