//! Single sign-on: `/api/v1/auth/oidc` (sign-in) and
//! `/api/v1/security/sso/providers` (configuration).

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_sso::{Provider, ProviderButton, ProviderInput, RoleMapping, SsoError, providers};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth_routes::signed_in;
use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::{ApiError, ApiResult, AppState};

pub fn sign_in_routes() -> Router<AppState> {
    Router::new()
        .route("/providers", get(list_buttons))
        .route("/start", post(start))
        .route("/callback", post(callback))
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_providers).post(create_provider))
        .route(
            "/{id}",
            get(get_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        .route("/{id}/role-mappings", put(set_role_mappings))
        .route("/{id}/scim", get(get_scim).delete(disable_scim))
        .route("/{id}/scim/token", post(new_scim_token))
}

pub(crate) fn map_sso(error: SsoError) -> ApiError {
    let failed = |slug: &str, title: &str, detail: &str| {
        ApiError::new(StatusCode::UNAUTHORIZED, slug, title).with_detail(detail)
    };
    match error {
        SsoError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        SsoError::ProviderNotFound => ApiError::not_found(),
        SsoError::NotConfigured => ApiError::unavailable(
            "Single sign-on needs auth.public_url, its redirect address, to be set.",
        ),
        SsoError::NoAccount => failed(
            "sso-no-account",
            "No Huxley account",
            "There's no Huxley account for this identity. Ask an administrator for access.",
        ),
        SsoError::DomainNotAllowed => failed(
            "sso-domain-not-allowed",
            "Address not allowed",
            "This sign-in option isn't set up for your email domain.",
        ),
        SsoError::UnverifiedEmail => failed(
            "sso-unverified-email",
            "Email not verified",
            "Your identity provider didn't confirm your email address.",
        ),
        SsoError::Disabled => failed(
            "sso-disabled",
            "Account disabled",
            "Your Huxley account is disabled.",
        ),
        SsoError::InvalidState | SsoError::Provider(_) | SsoError::MissingEmail => failed(
            "sso-failed",
            "Single sign-on failed",
            "The sign-in couldn't be completed. Try again.",
        ),
        other => ApiError::internal(&other),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

/// Enabled sign-in options, for the sign-in page.
#[utoipa::path(get, path = "/api/v1/auth/oidc/providers", tag = "auth",
    responses((status = 200, body = Vec<ProviderButton>)))]
pub async fn list_buttons(State(state): State<AppState>) -> ApiResult<Json<Vec<ProviderButton>>> {
    Ok(Json(state.sso.buttons().await.map_err(map_sso)?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartSsoRequest {
    /// The provider's key.
    pub provider: String,
    /// A same-site path to return to afterwards.
    pub next: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartSsoResponse {
    /// Send the browser here.
    pub authorization_url: String,
    /// Bind to the browser (e.g. an `HttpOnly` cookie) and check it equals the
    /// `state` the provider returns before calling `/callback`.
    pub state: String,
}

/// A same-site relative path, or nothing.
fn safe_next(next: Option<&str>) -> Option<&str> {
    next.filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\"))
}

/// Starts single sign-on. The provider redirects back to
/// `{auth.public_url}/login/oidc/callback` with `code` and `state`.
#[utoipa::path(post, path = "/api/v1/auth/oidc/start", tag = "auth",
    request_body = StartSsoRequest,
    responses(
        (status = 200, body = StartSsoResponse),
        (status = 404, description = "No such enabled provider", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn start(
    State(state): State<AppState>,
    Json(body): Json<StartSsoRequest>,
) -> ApiResult<Response> {
    let started = state
        .sso
        .start(&body.provider, safe_next(body.next.as_deref()))
        .await
        .map_err(map_sso)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(StartSsoResponse {
            authorization_url: started.authorization_url,
            state: started.state,
        }),
    )
        .into_response())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoCallbackRequest {
    pub state: String,
    pub code: String,
}

/// Completes single sign-on with the provider's `code` and `state`. Sets the
/// session cookie.
#[utoipa::path(post, path = "/api/v1/auth/oidc/callback", tag = "auth",
    request_body = SsoCallbackRequest,
    responses(
        (status = 200, description = "Signed in; session cookie set", body = crate::auth_routes::LoginResponse),
        (status = 401, description = "Sign-in refused; `type` says why", body = ApiError, content_type = "application/problem+json"),
        (status = 429, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn callback(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<SsoCallbackRequest>,
) -> ApiResult<Response> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let sign_in = state
        .sso
        .callback(&ctx, &body.state, &body.code)
        .await
        .map_err(map_sso)?;
    Ok(signed_in(&state, sign_in.session, sign_in.next))
}

async fn root_org(tx: &mut sqlx::PgConnection) -> ApiResult<Uuid> {
    let root = huxley_orgs::root(tx).await.map_err(db_err)?;
    root.map(|r| r.id)
        .ok_or_else(|| ApiError::unavailable("The instance has no root organization."))
}

/// A provider with its role mappings.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDetail {
    #[serde(flatten)]
    pub provider: Provider,
    pub role_mappings: Vec<RoleMapping>,
    /// Register this as the redirect URI at the provider.
    pub redirect_uri: Option<String>,
}

fn redirect_uri(state: &AppState) -> Option<String> {
    state
        .auth
        .public_url()
        .map(|u| format!("{}/login/oidc/callback", u.trim_end_matches('/')))
}

/// Configured identity providers.
#[utoipa::path(get, path = "/api/v1/security/sso/providers", tag = "security",
    responses((status = 200, body = Vec<Provider>), (status = 403, body = ApiError, content_type = "application/problem+json")))]
pub async fn list_providers(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Provider>>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let list = providers::list(&mut tx).await.map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(list))
}

/// One provider with its role mappings.
#[utoipa::path(get, path = "/api/v1/security/sso/providers/{id}", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    responses((status = 200, body = ProviderDetail), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProviderDetail>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let provider = providers::get(&mut tx, id).await.map_err(map_sso)?;
    let role_mappings = providers::role_mappings(&mut tx, id)
        .await
        .map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(ProviderDetail {
        provider,
        role_mappings,
        redirect_uri: redirect_uri(&state),
    }))
}

/// Adds an identity provider.
#[utoipa::path(post, path = "/api/v1/security/sso/providers", tag = "security",
    request_body = ProviderInput,
    responses((status = 201, body = Provider), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_provider(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<ProviderInput>,
) -> ApiResult<(StatusCode, Json<Provider>)> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let root = root_org(&mut tx).await?;
    let provider = providers::create(&mut tx, &state.crypto, root, &authed.user.ctx, &body)
        .await
        .map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok((StatusCode::CREATED, Json(provider)))
}

/// Replaces a provider's settings. Omit `clientSecret` to keep the stored one.
#[utoipa::path(put, path = "/api/v1/security/sso/providers/{id}", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    request_body = ProviderInput,
    responses((status = 200, body = Provider), (status = 404, body = ApiError, content_type = "application/problem+json"),
        (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<ProviderInput>,
) -> ApiResult<Json<Provider>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let root = root_org(&mut tx).await?;
    let provider = providers::update(&mut tx, &state.crypto, root, &authed.user.ctx, id, &body)
        .await
        .map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(provider))
}

/// Deletes a provider, the identities linked to it, and the roles its
/// mappings granted. Users are kept.
#[utoipa::path(delete, path = "/api/v1/security/sso/providers/{id}", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn delete_provider(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    providers::delete(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces a provider's claim → role mappings. Applied at each user's next
/// sign-in. The Owner role can't be mapped.
#[utoipa::path(put, path = "/api/v1/security/sso/providers/{id}/role-mappings", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    request_body = Vec<RoleMapping>,
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_role_mappings(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<Vec<RoleMapping>>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    providers::set_role_mappings(&mut tx, &authed.user.ctx, id, &body)
        .await
        .map_err(map_sso)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// SCIM provisioning for a provider (ADR-088): whether it's on, the base
/// URL to give the identity provider, and what it has provisioned.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScimSettings {
    #[serde(flatten)]
    pub status: huxley_sso::scim::ScimStatus,
    /// The SCIM base URL (needs `auth.public_url`).
    pub url: Option<String>,
}

/// A new SCIM token: shown once.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScimToken {
    pub token: String,
    pub url: Option<String>,
}

fn scim_url(state: &AppState) -> Option<String> {
    state
        .auth
        .public_url()
        .map(|u| format!("{}/scim/v2", u.trim_end_matches('/')))
}

fn map_scim(e: huxley_sso::scim::ScimError) -> ApiError {
    match e {
        huxley_sso::scim::ScimError::NotFound => ApiError::not_found(),
        other => ApiError::internal(&other),
    }
}

#[utoipa::path(get, path = "/api/v1/security/sso/providers/{id}/scim", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    responses((status = 200, body = ScimSettings), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_scim(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ScimSettings>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let status = huxley_sso::scim::status(&mut tx, id)
        .await
        .map_err(map_scim)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(ScimSettings {
        status,
        url: scim_url(&state),
    }))
}

/// Turns SCIM on with a new token, replacing any old one (which stops
/// working). The token is shown once.
#[utoipa::path(post, path = "/api/v1/security/sso/providers/{id}/scim/token", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    responses((status = 200, body = ScimToken), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn new_scim_token(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ScimToken>> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let token = huxley_sso::scim::new_token(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_scim)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(ScimToken {
        token,
        url: scim_url(&state),
    }))
}

/// Turns SCIM off: the token stops working. Provisioned users and teams stay.
#[utoipa::path(delete, path = "/api/v1/security/sso/providers/{id}/scim", tag = "security",
    params(("id" = Uuid, Path, description = "Provider id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn disable_scim(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authed.require_instance(permissions::SECURITY_POLICY_MANAGE)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    huxley_sso::scim::disable(&mut tx, &authed.user.ctx, id)
        .await
        .map_err(map_scim)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
