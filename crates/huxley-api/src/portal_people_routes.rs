//! A company's people, for its client admins in the portal (ADR-040):
//! their colleagues (the company's contacts), who has portal access, adding
//! a colleague, inviting them, and changing or ending their access. Only
//! the admin's own company; never their own access.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::accounts::{self, Person, Status};
use huxley_portal::{Perm, Role, signin};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::portal_routes::{PortalAuthed, map, send_link};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/people", get(list_people).post(add_person))
        .route(
            "/people/{contact}/access",
            post(give_access).patch(change_access),
        )
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn allowed(authed: &PortalAuthed) -> ApiResult<()> {
    if authed.principal.can(Perm::ManageContacts) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only your organization's admins manage people",
        ))
    }
}

/// Your company's people, with their portal access.
#[utoipa::path(get, path = "/api/portal/v1/people", operation_id = "portal_people", tag = "portal",
    responses((status = 200, body = Vec<Person>)))]
pub async fn list_people(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<Vec<Person>>> {
    allowed(&authed)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let people = accounts::people(&mut tx, authed.principal.org_id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(people))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortalNewPerson {
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    pub email: String,
    /// Give them portal access with this role (and email an invitation).
    #[serde(default)]
    pub role: Option<Role>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalPersonResult {
    pub contact_id: Uuid,
    /// An invitation was emailed.
    pub invitation_sent: bool,
}

/// Gives a contact access and emails an invitation (when email is set up).
async fn invite(
    state: &AppState,
    authed: &PortalAuthed,
    contact: Uuid,
    role: Role,
) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let account = accounts::invite(&mut tx, &authed.ctx, contact, role)
        .await
        .map_err(map)?;
    let secret = signin::issue_link(&mut tx, account.id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(send_link(state, account.id, &account.email, &secret, true).await)
}

/// A contact of your company (404 for anyone else's).
async fn colleague(state: &AppState, authed: &PortalAuthed, contact: Uuid) -> ApiResult<Person> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let people = accounts::people(&mut tx, authed.principal.org_id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    people
        .into_iter()
        .find(|p| p.contact_id == contact)
        .ok_or_else(ApiError::not_found)
}

/// Adds a colleague as a contact of your company, optionally with portal
/// access.
#[utoipa::path(post, path = "/api/portal/v1/people", operation_id = "portal_add_person", tag = "portal",
    request_body = PortalNewPerson,
    responses((status = 201, body = PortalPersonResult), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_person(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<PortalNewPerson>,
) -> ApiResult<(StatusCode, Json<PortalPersonResult>)> {
    allowed(&authed)?;
    let email = body.email.trim().to_owned();
    if !email.contains('@') {
        return Err(ApiError::validation().with_field_error("/email", "an email address"));
    }
    if body.first_name.trim().is_empty() {
        return Err(ApiError::validation().with_field_error("/firstName", "their first name"));
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM crm_contacts WHERE org_id = $1 AND lower(email) = lower($2) AND archived_at IS NULL)",
    )
    .bind(authed.principal.org_id)
    .bind(&email)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    if taken {
        return Err(ApiError::conflict(
            "Someone at your organization already has that address.",
        ));
    }
    let input: huxley_crm::contacts::ContactInput = serde_json::from_value(serde_json::json!({
        "firstName": body.first_name.trim(), "lastName": body.last_name.trim(), "email": email,
    }))
    .map_err(db_err)?;
    let contact =
        huxley_crm::contacts::create(&mut tx, &authed.ctx, authed.principal.org_id, &input)
            .await
            .map_err(|e| ApiError::validation().with_field_error("/email", e.to_string()))?;
    tx.commit().await.map_err(db_err)?;
    let invitation_sent = match body.role {
        Some(role) => invite(&state, &authed, contact.id, role).await?,
        None => false,
    };
    Ok((
        StatusCode::CREATED,
        Json(PortalPersonResult {
            contact_id: contact.id,
            invitation_sent,
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortalGiveAccess {
    pub role: Role,
}

/// Gives a colleague portal access (or re-invites them) and emails an
/// invitation.
#[utoipa::path(post, path = "/api/portal/v1/people/{contact}/access", operation_id = "portal_give_access", tag = "portal",
    params(("contact" = Uuid, Path)), request_body = PortalGiveAccess,
    responses((status = 200, body = PortalPersonResult)))]
pub async fn give_access(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(contact): Path<Uuid>,
    Json(body): Json<PortalGiveAccess>,
) -> ApiResult<Json<PortalPersonResult>> {
    allowed(&authed)?;
    colleague(&state, &authed, contact).await?;
    if contact == authed.principal.contact_id {
        return Err(ApiError::validation().with_field_error("/contact", "not your own access"));
    }
    let invitation_sent = invite(&state, &authed, contact, body.role).await?;
    Ok(Json(PortalPersonResult {
        contact_id: contact,
        invitation_sent,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortalChangeAccess {
    pub role: Role,
    pub status: Status,
    #[serde(default)]
    pub version: Option<i32>,
}

/// Changes a colleague's role, or ends (or restores) their access.
#[utoipa::path(patch, path = "/api/portal/v1/people/{contact}/access", operation_id = "portal_change_access", tag = "portal",
    params(("contact" = Uuid, Path)), request_body = PortalChangeAccess,
    responses((status = 200, body = Person), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn change_access(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Path(contact): Path<Uuid>,
    Json(body): Json<PortalChangeAccess>,
) -> ApiResult<Json<Person>> {
    allowed(&authed)?;
    let person = colleague(&state, &authed, contact).await?;
    if contact == authed.principal.contact_id {
        return Err(ApiError::validation().with_field_error("/contact", "not your own access"));
    }
    let account = person.account_id.ok_or_else(ApiError::not_found)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    accounts::update(
        &mut tx,
        &authed.ctx,
        account,
        body.role,
        body.status,
        body.version,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(colleague(&state, &authed, contact).await?))
}
