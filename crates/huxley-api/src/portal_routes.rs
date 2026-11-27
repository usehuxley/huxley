//! `/api/portal/v1`: the client portal's own API (ADR-040). Portal sessions
//! only (cookie `hx_portal_session`); staff sessions and API tokens never
//! authenticate here, and portal sessions never authenticate elsewhere.

use axum::extract::{FromRequestParts, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use http::request::Parts;
use huxley_core::RequestContext;
use huxley_db::{OrgScope, begin_scoped};
use huxley_portal::mfa;
use huxley_portal::signin::{self, NewSession, SignedIn};
use huxley_portal::{PortalError, PortalPrincipal};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::context::ClientInfo;
use crate::{ApiError, ApiResult, AppState};

/// The portal's session cookie.
pub const COOKIE_NAME: &str = "hx_portal_session";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/link", post(request_link))
        .route("/auth/redeem", post(redeem_link))
        .route("/auth/password", post(password_sign_in))
        .route("/auth/logout", post(sign_out))
        .route("/me", get(me))
        .route("/branding", get(branding))
        .route("/me/password", put(set_password))
        .route("/auth/mfa", post(complete_mfa))
        .route("/me/mfa", get(mfa_status))
        .route("/me/mfa/totp", post(begin_totp))
        .route("/me/mfa/totp/confirm", post(confirm_totp))
        .route("/me/mfa/totp/disable", post(disable_totp))
        .route("/me/mfa/recovery-codes", post(regenerate_recovery_codes))
}

pub(crate) fn map(e: PortalError) -> ApiError {
    match e {
        PortalError::NotFound => ApiError::not_found(),
        PortalError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        PortalError::Conflict(message) => ApiError::conflict(message),
        PortalError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        PortalError::BadCredentials => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad-credentials",
            "That didn't work",
        )
        .with_detail("The email or password is wrong, or the link is used or expired."),
        PortalError::BadCode => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad-code",
            "That code didn't work",
        )
        .with_detail("Check the code in your authenticator app, or sign in again."),
        PortalError::Locked(seconds) => {
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "locked", "Too many attempts")
                .with_detail(format!("Try again in {seconds} seconds."))
        }
        PortalError::SignIn(reason) => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "sso-failed",
            "Signing in with your organization didn't work",
        )
        .with_detail(match reason {
            "no_account" => {
                "You don't have access to this portal yet. Ask your IT contact or the service desk."
                    .to_owned()
            }
            "domain" => "That account's email address isn't one this sign-in is for.".to_owned(),
            "state" => "The sign-in took too long or was already used. Try again.".to_owned(),
            other => format!("Try again, or use a sign-in link instead ({other})."),
        }),
        PortalError::Db(e) => ApiError::internal(&e),
        PortalError::Sqlx(e) => ApiError::internal(&e),
    }
}

fn db_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&e)
}

fn token_from(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value.to_owned())
}

/// The signed-in portal account. Rejects with 401 without a live portal
/// session.
#[derive(Debug, Clone)]
pub struct PortalAuthed {
    pub principal: PortalPrincipal,
    /// For audit records: the portal account is named in their details.
    pub ctx: RequestContext,
}

impl PortalAuthed {
    /// The database scope for this account: its company only.
    pub fn scope(&self) -> OrgScope {
        OrgScope::Orgs(vec![self.principal.org_id])
    }
}

impl FromRequestParts<AppState> for PortalAuthed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let unauthorized = || {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "Sign in to the portal",
            )
        };
        let secret = token_from(parts).ok_or_else(unauthorized)?;
        let ClientInfo(ctx) = ClientInfo::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|never| match never {});
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(db_err)?;
        let principal = signin::authenticate(&mut tx, &secret)
            .await
            .map_err(map)?
            .ok_or_else(unauthorized)?;
        tx.commit().await.map_err(db_err)?;
        if principal.mfa_setup_required && !setup_allowed(parts.uri.path()) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "mfa-setup-required",
                "Set up two-factor sign-in first",
            )
            .with_detail("Your organization requires two-factor sign-in for the portal."));
        }
        if !huxley_portal_module_on(state, principal.org_id).await? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "module-disabled",
                "The portal is turned off",
            ));
        }
        Ok(Self { principal, ctx })
    }
}

/// What a session that must set up two-factor sign-in can still reach.
fn setup_allowed(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path.ends_with("/me") || path.contains("/me/mfa") || path.ends_with("/auth/logout")
}

async fn huxley_portal_module_on(state: &AppState, org_id: uuid::Uuid) -> ApiResult<bool> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let on = huxley_modules::is_active_at(&mut tx, huxley_portal::MODULE, org_id)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(on)
}

fn set_cookie(secret: &str, max_age: time::Duration, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={secret}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{secure}",
        max_age.whole_seconds()
    ))
    .expect("valid header characters")
}

fn clear_cookie(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}"
    ))
    .expect("valid header characters")
}

/// The signed-in account, as the portal shows it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalMe {
    #[serde(flatten)]
    pub principal: PortalPrincipal,
    /// Portal permissions, e.g. `portal.ticket:read:company`.
    pub permissions: Vec<&'static str>,
    /// How the portal looks for their company.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branding: Option<huxley_portal::branding::Branding>,
    /// The portal's parts that are on for their company: `servicedesk`
    /// (tickets), `knowledge` (help articles), `cmdb` (devices), `projects`,
    /// `sales` (quotes).
    pub modules: Vec<&'static str>,
}

/// Modules with a part in the portal.
const PORTAL_MODULES: &[&str] = &[
    huxley_servicedesk::MODULE,
    huxley_knowledge::MODULE,
    huxley_cmdb::MODULE,
    crate::project_routes::MODULE,
    huxley_sales::MODULE,
];

impl From<PortalPrincipal> for PortalMe {
    fn from(principal: PortalPrincipal) -> Self {
        let permissions = principal.role.permissions();
        Self {
            principal,
            permissions,
            branding: None,
            modules: Vec::new(),
        }
    }
}

pub(crate) fn signed_in_response(state: &AppState, session: NewSession) -> Response {
    (
        [
            (
                header::SET_COOKIE,
                set_cookie(&session.secret, session.max_age, state.config.cookie_secure),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Json(PortalMe::from(session.principal)),
    )
        .into_response()
}

fn ip_text(ctx: &RequestContext) -> Option<String> {
    ctx.ip.map(|ip| ip.to_string())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalLinkRequest {
    pub email: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PortalLinkResponse {
    /// Always true: whether an account exists isn't revealed.
    pub sent: bool,
}

/// Emails a sign-in link to whoever has portal access as this address. The
/// answer is the same whether or not anyone does.
#[utoipa::path(post, path = "/api/portal/v1/auth/link", operation_id = "portal_request_link", tag = "portal",
    request_body = PortalLinkRequest, responses((status = 202, body = PortalLinkResponse)))]
pub async fn request_link(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalLinkRequest>,
) -> ApiResult<(StatusCode, Json<PortalLinkResponse>)> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let found = signin::request_link(&mut tx, &ctx, &body.email)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    if let Some((account, address, secret)) = found {
        send_link(&state, account, &address, &secret, false).await;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(PortalLinkResponse { sent: true }),
    ))
}

/// The portal's URL for an account: its company's portal domain (set at
/// the MSP or the client), else `portal_public_url`.
async fn portal_url_for(state: &AppState, account: uuid::Uuid) -> Option<String> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System).await.ok()?;
    let org: uuid::Uuid = sqlx::query_scalar("SELECT org_id FROM portal_accounts WHERE id = $1")
        .bind(account)
        .fetch_optional(&mut *tx)
        .await
        .ok()??;
    let domain = huxley_portal::branding::base_url(&mut tx, org)
        .await
        .ok()
        .flatten();
    tx.commit().await.ok()?;
    domain.or_else(|| state.config.portal_public_url.clone())
}

/// Emails a portal sign-in link, when email and a portal URL are set up.
/// Returns whether it was sent.
pub(crate) async fn send_link(
    state: &AppState,
    account: uuid::Uuid,
    to: &str,
    secret: &str,
    invitation: bool,
) -> bool {
    let Some(base) = portal_url_for(state, account).await else {
        return false;
    };
    let mailer = state.auth.mailer();
    if !mailer.enabled() {
        return false;
    }
    let link = format!("{}/auth/link?token={secret}", base.trim_end_matches('/'));
    let (subject, body) = if invitation {
        (
            "You're invited to the support portal",
            "You can now see and raise your support requests in the portal. The link signs you in, works once, and lasts 15 minutes; ask for a new one from the portal's sign-in page any time.",
        )
    } else {
        (
            "Your support portal sign-in link",
            "Use this link to sign in. It works once, for 15 minutes.",
        )
    };
    mailer.send_later(huxley_email::action_email(
        to,
        subject,
        "Hello,",
        body,
        "Sign in",
        &link,
        "If you didn't ask for this, you can ignore this email.",
    ));
    true
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalRedeemRequest {
    pub token: String,
}

/// Signs in with an emailed link (once). Sets the portal session cookie.
#[utoipa::path(post, path = "/api/portal/v1/auth/redeem", operation_id = "portal_redeem_link", tag = "portal",
    request_body = PortalRedeemRequest,
    responses(
        (status = 200, body = PortalMe),
        (status = 202, description = "A two-factor code is needed", body = PortalMfaRequired),
        (status = 401, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn redeem_link(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalRedeemRequest>,
) -> ApiResult<Response> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ip = ip_text(&ctx);
    let signed_in = signin::redeem(
        &mut tx,
        &ctx,
        &body.token,
        ip.as_deref(),
        ctx.user_agent.as_deref(),
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(signed_in_or_challenge(&state, signed_in))
}

/// A code is needed: finish with `POST /api/portal/v1/auth/mfa`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PortalMfaRequired {
    pub challenge: String,
}

fn signed_in_or_challenge(state: &AppState, signed_in: SignedIn) -> Response {
    match signed_in {
        SignedIn::Session(session) => signed_in_response(state, session),
        SignedIn::MfaRequired { challenge } => (
            StatusCode::ACCEPTED,
            [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
            Json(PortalMfaRequired { challenge }),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalPasswordSignIn {
    pub email: String,
    pub password: String,
}

/// Signs in with an email and password. Sets the portal session cookie.
#[utoipa::path(post, path = "/api/portal/v1/auth/password", operation_id = "portal_password_sign_in", tag = "portal",
    request_body = PortalPasswordSignIn,
    responses(
        (status = 200, body = PortalMe),
        (status = 202, description = "A two-factor code is needed", body = PortalMfaRequired),
        (status = 401, body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn password_sign_in(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalPasswordSignIn>,
) -> ApiResult<Response> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ip = ip_text(&ctx);
    let result = signin::sign_in_with_password(
        &mut tx,
        &ctx,
        &body.email,
        &body.password,
        ip.as_deref(),
        ctx.user_agent.as_deref(),
    )
    .await;
    // Failed attempts are counted even when signing in fails.
    tx.commit().await.map_err(db_err)?;
    Ok(signed_in_or_challenge(&state, result.map_err(map)?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalMfaCode {
    pub challenge: String,
    /// From the authenticator app, or a recovery code.
    pub code: String,
}

/// Finishes signing in with a two-factor code. Sets the portal session
/// cookie.
#[utoipa::path(post, path = "/api/portal/v1/auth/mfa", operation_id = "portal_complete_mfa", tag = "portal",
    request_body = PortalMfaCode,
    responses((status = 200, body = PortalMe), (status = 401, body = ApiError, content_type = "application/problem+json")))]
pub async fn complete_mfa(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<PortalMfaCode>,
) -> ApiResult<Response> {
    crate::auth_routes::rate_limit(&state, &ctx)?;
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let ip = ip_text(&ctx);
    let result = mfa::complete(
        &mut tx,
        &state.crypto,
        &ctx,
        &body.challenge,
        &body.code,
        ip.as_deref(),
        ctx.user_agent.as_deref(),
    )
    .await;
    // Wrong codes are counted even when signing in fails.
    tx.commit().await.map_err(db_err)?;
    Ok(signed_in_response(&state, result.map_err(map)?))
}

/// Your two-factor sign-in.
#[utoipa::path(get, path = "/api/portal/v1/me/mfa", operation_id = "portal_mfa_status", tag = "portal",
    responses((status = 200, body = mfa::Status)))]
pub async fn mfa_status(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<mfa::Status>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let status = mfa::status(&mut tx, &authed.principal).await.map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(status))
}

/// Starts setting up an authenticator app.
#[utoipa::path(post, path = "/api/portal/v1/me/mfa/totp", operation_id = "portal_begin_totp", tag = "portal",
    responses((status = 200, body = mfa::Enrollment), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn begin_totp(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<mfa::Enrollment>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let issuer = huxley_portal::branding::at(&mut tx, authed.principal.org_id)
        .await
        .map_err(map)?
        .name;
    let enrollment = mfa::begin(
        &mut tx,
        &state.crypto,
        &authed.ctx,
        &authed.principal,
        &issuer,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(enrollment))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalTotpCode {
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortalRecoveryCodes {
    /// Shown once: each signs in once without the app.
    pub recovery_codes: Vec<String>,
}

/// Finishes setting up with a code from the app. Returns recovery codes
/// (shown once) and signs out your other sessions.
#[utoipa::path(post, path = "/api/portal/v1/me/mfa/totp/confirm", operation_id = "portal_confirm_totp", tag = "portal",
    request_body = PortalTotpCode,
    responses((status = 200, body = PortalRecoveryCodes), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn confirm_totp(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<PortalTotpCode>,
) -> ApiResult<Json<PortalRecoveryCodes>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let recovery_codes = mfa::confirm(
        &mut tx,
        &state.crypto,
        &authed.ctx,
        &authed.principal,
        &body.code,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(PortalRecoveryCodes { recovery_codes }))
}

/// Turns off two-factor sign-in, with a current code (not while your
/// organization requires it).
#[utoipa::path(post, path = "/api/portal/v1/me/mfa/totp/disable", operation_id = "portal_disable_totp", tag = "portal",
    request_body = PortalTotpCode,
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn disable_totp(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<PortalTotpCode>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    mfa::disable(
        &mut tx,
        &state.crypto,
        &authed.ctx,
        &authed.principal,
        &body.code,
    )
    .await
    .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces your recovery codes (the old ones stop working).
#[utoipa::path(post, path = "/api/portal/v1/me/mfa/recovery-codes", operation_id = "portal_regenerate_recovery_codes",
    tag = "portal", responses((status = 200, body = PortalRecoveryCodes)))]
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    authed: PortalAuthed,
) -> ApiResult<Json<PortalRecoveryCodes>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let recovery_codes = mfa::regenerate_recovery_codes(&mut tx, &authed.ctx, &authed.principal)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(PortalRecoveryCodes { recovery_codes }))
}

/// Signs out of the portal.
#[utoipa::path(post, path = "/api/portal/v1/auth/logout", operation_id = "portal_sign_out", tag = "portal",
    responses((status = 204)))]
pub async fn sign_out(
    State(state): State<AppState>,
    authed: Option<PortalAuthed>,
) -> ApiResult<Response> {
    if let Some(authed) = authed {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(db_err)?;
        signin::sign_out(&mut tx, authed.principal.session_id)
            .await
            .map_err(map)?;
        tx.commit().await.map_err(db_err)?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, clear_cookie(state.config.cookie_secure))],
    )
        .into_response())
}

impl axum::extract::OptionalFromRequestParts<AppState> for PortalAuthed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        match <Self as FromRequestParts<AppState>>::from_request_parts(parts, state).await {
            Ok(authed) => Ok(Some(authed)),
            Err(e) if e.status == StatusCode::UNAUTHORIZED => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Who's signed in to the portal, with their company and permissions.
#[utoipa::path(get, path = "/api/portal/v1/me", operation_id = "portal_me", tag = "portal",
    responses((status = 200, body = PortalMe), (status = 401, body = ApiError, content_type = "application/problem+json")))]
pub async fn me(State(state): State<AppState>, authed: PortalAuthed) -> ApiResult<Json<PortalMe>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let branding = huxley_portal::branding::at(&mut tx, authed.principal.org_id)
        .await
        .map_err(map)?;
    let mut modules = Vec::new();
    for module in PORTAL_MODULES {
        if huxley_modules::is_active_at(&mut tx, module, authed.principal.org_id)
            .await
            .map_err(db_err)?
        {
            modules.push(*module);
        }
    }
    tx.commit().await.map_err(db_err)?;
    let mut me = PortalMe::from(authed.principal);
    me.branding = Some(branding);
    me.modules = modules;
    Ok(Json(me))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BrandingParams {
    /// The portal's host name, e.g. `support.example.com`.
    pub host: Option<String>,
}

/// How the portal looks before anyone signs in: the branding of the
/// organization whose portal domain this is (else the MSP's).
#[utoipa::path(get, path = "/api/portal/v1/branding", operation_id = "portal_branding", tag = "portal",
    params(BrandingParams), responses((status = 200, body = huxley_portal::branding::Branding)))]
pub async fn branding(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<BrandingParams>,
) -> ApiResult<Json<huxley_portal::branding::Branding>> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    let org = huxley_portal::branding::org_for_host(&mut tx, params.host.as_deref())
        .await
        .map_err(map)?;
    let branding = match org {
        Some(org) => huxley_portal::branding::at(&mut tx, org)
            .await
            .map_err(map)?,
        None => huxley_portal::branding::Branding {
            name: "Support portal".into(),
            accent_color: "#4f46e5".into(),
            logo: None,
            support_email: None,
        },
    };
    tx.commit().await.map_err(db_err)?;
    Ok(Json(branding))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PortalSetPassword {
    pub password: String,
}

/// Sets your portal password (your other sessions end).
#[utoipa::path(put, path = "/api/portal/v1/me/password", operation_id = "portal_set_password", tag = "portal",
    request_body = PortalSetPassword,
    responses((status = 204), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_password(
    State(state): State<AppState>,
    authed: PortalAuthed,
    Json(body): Json<PortalSetPassword>,
) -> ApiResult<StatusCode> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(db_err)?;
    signin::set_password(&mut tx, &authed.ctx, &authed.principal, &body.password)
        .await
        .map_err(map)?;
    tx.commit().await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}
