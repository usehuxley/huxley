//! Staff managing contacts' portal access (ADR-040): `crm:read` at the
//! company to see who has it, `crm:manage` to invite, change roles and
//! disable. The `portal` module must be on there.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::accounts::{self, PortalAccount, Status};
use huxley_portal::{Role, signin};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module_at;
use crate::portal_routes::{map, send_link};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/orgs/{id}/portal-accounts", get(list_portal_accounts))
        .route("/contacts/{id}/portal-access", post(invite_contact))
        .route("/portal-accounts/{id}", patch(update_portal_account))
        .route("/portal-accounts/{id}/invitation", post(resend_invitation))
        .route("/portal-accounts/{id}/mfa-reset", post(reset_portal_mfa))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn org_of(state: &AppState, table: &str, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT org_id FROM {table} WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    org.ok_or_else(ApiError::not_found)
}

async fn allow(
    state: &AppState,
    authed: &Authed,
    permission: &str,
    org: Uuid,
) -> ApiResult<OrgScope> {
    require_module_at(state, huxley_portal::MODULE, org).await?;
    authed.require_at(state, permission, org).await?;
    authed.scope(state).await
}

/// An account, and whether an email went out.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalAccessResult {
    pub account: PortalAccount,
    /// Whether an invitation was emailed (needs email and the portal's URL
    /// set up).
    pub invitation_sent: bool,
}

/// A company's portal accounts.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/portal-accounts", operation_id = "list_portal_accounts",
    tag = "portal", params(("id" = Uuid, Path)), responses((status = 200, body = Vec<PortalAccount>)))]
pub async fn list_portal_accounts(
    State(state): State<AppState>,
    authed: Authed,
    Path(org): Path<Uuid>,
) -> ApiResult<Json<Vec<PortalAccount>>> {
    let scope = allow(&state, &authed, permissions::CRM_READ, org).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let all = accounts::list(&mut tx, org).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InvitePortalContact {
    pub role: Role,
}

/// Gives a contact portal access with a role, and emails them a sign-in
/// link.
#[utoipa::path(post, path = "/api/v1/contacts/{id}/portal-access", operation_id = "invite_portal_contact",
    tag = "portal", params(("id" = Uuid, Path)), request_body = InvitePortalContact,
    responses(
        (status = 201, body = PortalAccessResult),
        (status = 409, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn invite_contact(
    State(state): State<AppState>,
    authed: Authed,
    Path(contact): Path<Uuid>,
    Json(body): Json<InvitePortalContact>,
) -> ApiResult<(StatusCode, Json<PortalAccessResult>)> {
    let org = org_of(&state, "crm_contacts", contact).await?;
    let scope = allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let account = accounts::invite(&mut tx, &authed.user.ctx, contact, body.role)
        .await
        .map_err(map)?;
    let secret = signin::issue_link(&mut tx, account.id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let invitation_sent = send_link(&state, account.id, &account.email, &secret, true).await;
    Ok((
        StatusCode::CREATED,
        Json(PortalAccessResult {
            account,
            invitation_sent,
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePortalAccount {
    pub role: Role,
    pub status: Status,
    #[serde(default)]
    pub version: Option<i32>,
}

/// Changes an account's role, or disables (or re-enables) it.
#[utoipa::path(patch, path = "/api/v1/portal-accounts/{id}", operation_id = "update_portal_account",
    tag = "portal", params(("id" = Uuid, Path)), request_body = UpdatePortalAccount,
    responses((status = 200, body = PortalAccount), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_portal_account(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePortalAccount>,
) -> ApiResult<Json<PortalAccount>> {
    let org = org_of(&state, "portal_accounts", id).await?;
    let scope = allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let account = accounts::update(
        &mut tx,
        &authed.user.ctx,
        id,
        body.role,
        body.status,
        body.version,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(account))
}

/// Turns off a contact's two-factor sign-in (a lost phone). Their sessions
/// end; they set it up again if their company requires it.
#[utoipa::path(post, path = "/api/v1/portal-accounts/{id}/mfa-reset", operation_id = "reset_portal_mfa",
    tag = "portal", params(("id" = Uuid, Path)), responses((status = 200, body = PortalAccount)))]
pub async fn reset_portal_mfa(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalAccount>> {
    let org = org_of(&state, "portal_accounts", id).await?;
    let scope = allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    huxley_portal::mfa::reset(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map)?;
    let account = accounts::get(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(account))
}

/// Emails a new sign-in link to an invited or active account.
#[utoipa::path(post, path = "/api/v1/portal-accounts/{id}/invitation", operation_id = "resend_portal_invitation",
    tag = "portal", params(("id" = Uuid, Path)),
    responses((status = 200, body = PortalAccessResult), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn resend_invitation(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<PortalAccessResult>> {
    let org = org_of(&state, "portal_accounts", id).await?;
    let scope = allow(&state, &authed, permissions::CRM_MANAGE, org).await?;
    let mut tx = begin_scoped(&state.db, &scope).await.map_err(db_err)?;
    let account = accounts::get(&mut tx, id).await.map_err(map)?;
    if account.status == Status::Disabled {
        return Err(
            ApiError::validation().with_field_error("/status", "re-enable the account first")
        );
    }
    let secret = signin::issue_link(&mut tx, id).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let invitation_sent = send_link(
        &state,
        account.id,
        &account.email,
        &secret,
        account.status == Status::Invited,
    )
    .await;
    Ok(Json(PortalAccessResult {
        account,
        invitation_sent,
    }))
}
