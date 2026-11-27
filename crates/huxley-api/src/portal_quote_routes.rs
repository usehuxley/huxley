//! Quotes in the client portal (ADR-065): the quotes sent to the company
//! (all of them for approvers and admins; those addressed to them for
//! anyone), and answering one with a signature.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use huxley_sales::signing::{self, PortalAnswer, PortalQuote, Viewer};
use uuid::Uuid;

use crate::portal_routes::PortalAuthed;
use crate::{ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/quotes", get(list))
        .route("/quotes/{id}", get(get_one))
        .route("/quotes/{id}/answer", post(answer))
}

fn viewer(authed: &PortalAuthed) -> Viewer {
    Viewer {
        org_id: authed.principal.org_id,
        contact_id: authed.principal.contact_id,
        account_id: authed.principal.account_id,
        all: authed.principal.can(Perm::Quotes),
    }
}

/// Quotes for you or your company, newest first. Empty when sales is off.
#[utoipa::path(get, path = "/api/portal/v1/quotes", operation_id = "portal_quotes", tag = "portal",
    responses((status = 200, body = Vec<PortalQuote>)))]
pub async fn list(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<PortalQuote>>> {
    let who = viewer(&authed);
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    if !huxley_modules::is_active_at(&mut tx, huxley_sales::MODULE, who.org_id)
        .await
        .map_err(|e| crate::ApiError::internal(&e))?
    {
        return Ok(Json(Vec::new()));
    }
    tx.commit()
        .await
        .map_err(|e| crate::ApiError::internal(&e))?;
    Ok(Json(
        crate::sales_routes::scoped(&state, &authed.scope(), async |tx| {
            signing::portal_list(tx, who).await
        })
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/portal/v1/quotes/{id}", operation_id = "portal_quote", tag = "portal",
    params(("id" = Uuid, Path)), responses((status = 200, body = PortalQuote)))]
pub async fn get_one(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalQuote>> {
    let who = viewer(&authed);
    Ok(Json(
        crate::sales_routes::scoped(&state, &authed.scope(), async |tx| {
            signing::portal_get(tx, who, id).await
        })
        .await?,
    ))
}

/// Accepts (signed, with the options taken) or declines a quote.
#[utoipa::path(post, path = "/api/portal/v1/quotes/{id}/answer", operation_id = "portal_answer_quote",
    tag = "portal", params(("id" = Uuid, Path)), request_body = PortalAnswer,
    responses(
        (status = 200, body = PortalQuote),
        (status = 409, description = "Already answered, changed since, or expired", body = crate::ApiError, content_type = "application/problem+json"),
        (status = 422, body = crate::ApiError, content_type = "application/problem+json"),
    ))]
pub async fn answer(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
    Json(body): Json<PortalAnswer>,
) -> ApiResult<Json<PortalQuote>> {
    let who = viewer(&authed);
    let ctx = authed.ctx.clone();
    Ok(Json(
        crate::sales_routes::scoped(&state, &authed.scope(), async |tx| {
            signing::answer(tx, &ctx, who, id, &body).await
        })
        .await?,
    ))
}
