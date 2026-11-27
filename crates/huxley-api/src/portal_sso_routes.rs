//! Portal sign-in with a client's own identity provider (ADR-040).
//!
//! - Staff with `crm:manage` at a company (the `portal` module on) keep its
//!   identity providers: `/api/v1/orgs/{id}/portal-identity-providers`,
//!   `/api/v1/portal-identity-providers/{id}`.
//! - The portal starts and finishes sign-ins:
//!   `/api/portal/v1/auth/sso/start`, `/api/portal/v1/auth/sso/callback`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::sso::{self, Provider, ProviderInput};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::module_routes::require_module_at;
use crate::portal_routes::{map, signed_in_response};
use crate::{ApiError, ApiResult, AppState};

/// Staff routes (under `/api/v1`).
pub fn staff_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{id}/portal-identity-providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/portal-identity-providers/{id}",
            put(update_provider).delete(delete_provider),
        )
}

/// Portal routes (under `/api/portal/v1`).
pub fn portal_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/sso/start", post(start_sso))
        .route("/auth/sso/callback", post(finish_sso))
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

async fn allow(state: &AppState, authed: &Authed, org: Uuid) -> ApiResult<()> {
    require_module_at(state, huxley_portal::MODULE, org).await?;
    authed.require_at(state, permissions::CRM_MANAGE, org).await
}

async fn provider_org(state: &AppState, id: Uuid) -> ApiResult<Uuid> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org = sso::get(&mut tx, id).await.map_err(map)?.org_id;
    tx.commit().await.map_err(db_err)?;
    Ok(org)
}

/// A company's portal identity providers.
#[utoipa::path(get, path = "/api/v1/orgs/{id}/portal-identity-providers", operation_id = "list_portal_identity_providers",
    tag = "portal", params(("id" = Uuid, Path)), responses((status = 200, body = Vec<Provider>)))]
pub async fn list_providers(
    State(state): State<AppState>,
    authed: Authed,
    Path(org): Path<Uuid>,
) -> ApiResult<Json<Vec<Provider>>> {
    allow(&state, &authed, org).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let all = sso::list(&mut tx, org).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(all))
}

/// Adds an identity provider that signs the company's contacts in to the
/// portal (and those of the organizations below it).
#[utoipa::path(post, path = "/api/v1/orgs/{id}/portal-identity-providers", operation_id = "create_portal_identity_provider",
    tag = "portal", params(("id" = Uuid, Path)), request_body = ProviderInput,
    responses(
        (status = 201, body = Provider),
        (status = 409, description = "A domain is taken", body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn create_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(org): Path<Uuid>,
    Json(body): Json<ProviderInput>,
) -> ApiResult<(StatusCode, Json<Provider>)> {
    allow(&state, &authed, org).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let provider = sso::create(&mut tx, &state.crypto, &authed.user.ctx, org, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(provider)))
}

#[utoipa::path(put, path = "/api/v1/portal-identity-providers/{id}", operation_id = "update_portal_identity_provider",
    tag = "portal", params(("id" = Uuid, Path)), request_body = ProviderInput,
    responses((status = 200, body = Provider), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ProviderInput>,
) -> ApiResult<Json<Provider>> {
    let org = provider_org(&state, id).await?;
    allow(&state, &authed, org).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    let provider = sso::update(&mut tx, &state.crypto, &authed.user.ctx, id, &body)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(provider))
}

#[utoipa::path(delete, path = "/api/v1/portal-identity-providers/{id}", operation_id = "delete_portal_identity_provider",
    tag = "portal", params(("id" = Uuid, Path)), responses((status = 204)))]
pub async fn delete_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org = provider_org(&state, id).await?;
    allow(&state, &authed, org).await?;
    let mut tx = begin_scoped(&state.db, &authed.scope(&state).await?)
        .await
        .map_err(db_err)?;
    sso::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalSsoStart {
    pub email: String,
    /// The portal's callback page, e.g. `https://support.example.com/auth/sso/callback`.
    pub redirect: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalSsoStarted {
    /// The provider's name, e.g. "Contoso (Microsoft)".
    pub provider: String,
    /// Where to send the browser.
    pub url: String,
    /// Keep it in a cookie and send it back with the callback.
    pub state: String,
}

/// Whether `redirect` is this portal's callback page: on the provider
/// company's portal domain, or the configured portal URL.
async fn redirect_allowed(state: &AppState, org: Uuid, redirect: &str) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let base = huxley_portal::branding::base_url(&mut tx, org)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    let ok = |b: &str| redirect == format!("{}/auth/sso/callback", b.trim_end_matches('/'));
    Ok(
        base.as_deref().is_some_and(ok)
            || state.config.portal_public_url.as_deref().is_some_and(ok),
    )
}

/// Starts signing in with the identity provider for an email address's
/// domain. 404 when no provider signs that domain in (use a sign-in link).
#[utoipa::path(post, path = "/api/portal/v1/auth/sso/start", operation_id = "portal_sso_start", tag = "portal",
    request_body = PortalSsoStart,
    responses((status = 200, body = PortalSsoStarted), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn start_sso(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalSsoStart>,
) -> ApiResult<Json<PortalSsoStarted>> {
    crate::auth_routes::rate_limit(&state, &ctx).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let provider = sso::for_email(&mut tx, &body.email)
        .await
        .map_err(map)?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(db_err)?;
    if !redirect_allowed(&state, provider.org_id, &body.redirect).await? {
        return Err(
            ApiError::validation().with_field_error("/redirect", "not this portal's callback page")
        );
    }
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let (url, token) = sso::start(&mut tx, &state.sso, &provider, &body.redirect)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(PortalSsoStarted {
        provider: provider.name,
        url,
        state: token,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalSsoCallback {
    pub state: String,
    pub code: String,
}

/// Finishes signing in from the provider's callback. Sets the portal session
/// cookie.
#[utoipa::path(post, path = "/api/portal/v1/auth/sso/callback", operation_id = "portal_sso_callback", tag = "portal",
    request_body = PortalSsoCallback,
    responses((status = 200, body = crate::portal_routes::PortalMe), (status = 401, body = ApiError, content_type = "application/problem+json")))]
pub async fn finish_sso(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalSsoCallback>,
) -> ApiResult<Response> {
    crate::auth_routes::rate_limit(&state, &ctx).await?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ip = ctx.ip.map(|ip| ip.to_string());
    let result = sso::callback(
        &mut tx,
        &state.sso,
        &state.crypto,
        &ctx,
        &body.state,
        &body.code,
        ip.as_deref(),
        ctx.user_agent.as_deref(),
    )
    .await;
    match result {
        Ok(session) => {
            tx.commit().await.map_err(db_err)?;
            Ok(signed_in_response(&state, session))
        }
        Err(e) => {
            // The state is used up either way.
            tx.commit().await.map_err(db_err)?;
            Err(map(e))
        }
    }
}
