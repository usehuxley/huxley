//! `/api/portal/v1/tickets`: tickets in the client portal (ADR-040). Client
//! users see the tickets they asked for; client admins see their company's.
//! Public notes only. The service desk module must be on for the company.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::Perm;
use huxley_servicedesk::portal::{self, PortalNote, PortalTicket, Seen};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::module_routes::require_module_at;
use crate::portal_routes::PortalAuthed;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tickets", get(list_tickets).post(raise_ticket))
        .route("/tickets/{id}", get(get_ticket))
        .route("/tickets/{id}/replies", post(reply_to_ticket))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn desk(e: huxley_servicedesk::DeskError) -> ApiError {
    match e {
        huxley_servicedesk::DeskError::NotFound => ApiError::not_found(),
        huxley_servicedesk::DeskError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        other => ApiError::internal(&other),
    }
}

/// The service desk is on for the account's company; which tickets it sees.
async fn seen(state: &AppState, authed: &PortalAuthed, company: bool) -> ApiResult<Seen> {
    require_module_at(state, huxley_servicedesk::MODULE, authed.principal.org_id).await?;
    Ok(if company {
        if !authed.principal.can(Perm::TicketsCompany) {
            return Err(ApiError::forbidden());
        }
        Seen::Company
    } else if authed.principal.can(Perm::TicketsCompany) {
        // Admins see the company's tickets when looking at one.
        Seen::Company
    } else {
        Seen::Own(authed.principal.contact_id)
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PortalTicketParams {
    /// `open` (the default) or `closed` (resolved and closed).
    pub status: Option<String>,
    /// `mine` (the default) or `company` (client admins).
    pub scope: Option<String>,
    /// Words in the title, or the reference.
    pub q: Option<String>,
}

/// Your tickets (or, for client admins with `scope=company`, your
/// company's).
#[utoipa::path(get, path = "/api/portal/v1/tickets", operation_id = "portal_list_tickets", tag = "portal",
    params(PortalTicketParams), responses((status = 200, body = Vec<PortalTicket>)))]
pub async fn list_tickets(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Query(params): Query<PortalTicketParams>,
) -> ApiResult<Json<Vec<PortalTicket>>> {
    let company = params.scope.as_deref() == Some("company");
    let seen = if company {
        seen(&state, &authed, true).await?
    } else {
        require_module_at(&state, huxley_servicedesk::MODULE, authed.principal.org_id).await?;
        Seen::Own(authed.principal.contact_id)
    };
    let open = params.status.as_deref() != Some("closed");
    let mut tx = begin_scoped(&state.db, &authed.scope())
        .await
        .map_err(db_err)?;
    let tickets = portal::list(
        &mut tx,
        authed.principal.org_id,
        seen,
        open,
        params.q.as_deref(),
    )
    .await
    .map_err(desk)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(tickets))
}

/// A ticket with its public notes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalTicketView {
    #[serde(flatten)]
    pub ticket: PortalTicket,
    pub notes: Vec<PortalNote>,
    /// Whether you can reply (it isn't closed).
    pub can_reply: bool,
}

async fn view(state: &AppState, authed: &PortalAuthed, id: Uuid) -> ApiResult<PortalTicketView> {
    let seen = seen(state, authed, false).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope())
        .await
        .map_err(db_err)?;
    let ticket = portal::get(&mut tx, authed.principal.org_id, seen, id)
        .await
        .map_err(desk)?;
    let notes = portal::notes(&mut tx, id).await.map_err(desk)?;
    tx.commit().await.map_err(db_err)?;
    let can_reply = ticket.status_category != "closed";
    Ok(PortalTicketView {
        ticket,
        notes,
        can_reply,
    })
}

#[utoipa::path(get, path = "/api/portal/v1/tickets/{id}", operation_id = "portal_get_ticket", tag = "portal",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = PortalTicketView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_ticket(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalTicketView>> {
    Ok(Json(view(&state, &authed, id).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RaisePortalTicket {
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// Raises a ticket for you, at your company.
#[utoipa::path(post, path = "/api/portal/v1/tickets", operation_id = "portal_raise_ticket", tag = "portal",
    request_body = RaisePortalTicket,
    responses((status = 201, body = PortalTicketView), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn raise_ticket(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<RaisePortalTicket>,
) -> ApiResult<(StatusCode, Json<PortalTicketView>)> {
    require_module_at(&state, huxley_servicedesk::MODULE, authed.principal.org_id).await?;
    // Numbering and SLA terms come from above the company: system scope,
    // for the account's own company only.
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ticket = portal::create(
        &mut tx,
        &authed.ctx,
        authed.principal.org_id,
        authed.principal.contact_id,
        &body.title,
        &body.description,
    )
    .await
    .map_err(desk)?;
    tx.commit().await.map_err(db_err)?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state, &authed, ticket.id).await?),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalReply {
    pub body: String,
}

/// Replies to a ticket you see. A resolved ticket reopens.
#[utoipa::path(post, path = "/api/portal/v1/tickets/{id}/replies", operation_id = "portal_reply_to_ticket",
    tag = "portal", params(("id" = Uuid, Path)), request_body = PortalReply,
    responses((status = 201, body = PortalNote), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn reply_to_ticket(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(id): Path<Uuid>,
    Json(body): Json<PortalReply>,
) -> ApiResult<(StatusCode, Json<PortalNote>)> {
    let seen = seen(&state, &authed, false).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ticket = portal::get(&mut tx, authed.principal.org_id, seen, id)
        .await
        .map_err(desk)?;
    let note = portal::reply(
        &mut tx,
        &authed.ctx,
        &ticket,
        authed.principal.contact_id,
        &body.body,
    )
    .await
    .map_err(desk)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(note)))
}
