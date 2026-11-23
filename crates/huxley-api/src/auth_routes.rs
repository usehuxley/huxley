//! `/api/v1/auth`: sign in, sign out, current user, and session management.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use huxley_auth::{
    AuthError, LoginOutcome, LoginSuccess, MfaMethod, SecondFactor, Session, User, UserStatus,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::authz::Authed;
use crate::context::ClientInfo;
use crate::session::{self, CurrentUser};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/login/mfa", post(login_mfa))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/password", post(change_password))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", delete(revoke_session))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    #[schema(format = Password)]
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserView {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    /// `active` or `disabled`.
    pub status: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_login_at: Option<OffsetDateTime>,
    /// Has a second factor (authenticator app) set up.
    pub mfa_enabled: bool,
}

impl From<User> for UserView {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            display_name: user.display_name,
            status: match user.status {
                UserStatus::Active => "active",
                UserStatus::Disabled => "disabled",
            }
            .to_owned(),
            last_login_at: user.last_login_at,
            mfa_enabled: user.mfa_enabled,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    /// True for the session making this request.
    pub current: bool,
}

impl SessionView {
    fn new(session: Session, current_id: Uuid) -> Self {
        Self {
            current: session.id == current_id,
            id: session.id,
            created_at: session.created_at,
            last_seen_at: session.last_seen_at,
            expires_at: session.absolute_expires_at,
            ip: session.ip,
            user_agent: session.user_agent,
        }
    }
}

pub(crate) fn map_auth_error(error: AuthError) -> ApiError {
    match error {
        AuthError::InvalidMfaChallenge => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "mfa-challenge-expired",
            "The sign-in attempt has expired",
        )
        .with_detail("Sign in again with your email and password."),
        AuthError::InvalidMfaCode => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid-mfa-code",
            "Invalid authentication code",
        ),
        AuthError::MfaAlreadyEnabled => {
            ApiError::conflict("An authenticator app is already set up. Turn it off first.")
        }
        AuthError::MfaNotEnabled => ApiError::conflict("No authenticator app is set up."),
        AuthError::NoPendingEnrollment => {
            ApiError::conflict("Start authenticator setup before confirming it.")
        }
        AuthError::WrongCurrentPassword => {
            ApiError::validation().with_field_error("/password", "The password is incorrect.")
        }
        AuthError::InvalidCredentials => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid-credentials",
            "Invalid email or password",
        ),
        AuthError::SessionNotFound => ApiError::not_found(),
        AuthError::WeakPassword(e) => {
            ApiError::validation().with_field_error("/newPassword", e.to_string())
        }
        other => ApiError::internal(&other),
    }
}

/// Second-factor methods offered for a challenge.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum MfaMethodView {
    Totp,
    RecoveryCode,
}

impl From<MfaMethod> for MfaMethodView {
    fn from(method: MfaMethod) -> Self {
        match method {
            MfaMethod::Totp => Self::Totp,
            MfaMethod::RecoveryCode => Self::RecoveryCode,
        }
    }
}

/// The result of a sign-in step.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LoginResponse {
    /// Signed in; the session cookie is set.
    SignedIn { user: UserView },
    /// The password was right; finish with `POST /api/v1/auth/login/mfa`.
    #[serde(rename_all = "camelCase")]
    MfaRequired {
        /// Pass to `/login/mfa`. Valid for a few minutes and a few attempts.
        challenge: String,
        #[serde(with = "time::serde::rfc3339")]
        expires_at: OffsetDateTime,
        methods: Vec<MfaMethodView>,
    },
}

fn rate_limit(state: &AppState, ctx: &huxley_core::RequestContext) -> ApiResult<()> {
    if let Some(ip) = ctx.ip
        && !state.login_limiter.check(ip)
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate-limited",
            "Too many sign-in attempts",
        )
        .with_detail("Wait a minute and try again."));
    }
    Ok(())
}

fn signed_in(state: &AppState, success: LoginSuccess) -> Response {
    let cookie = session::set_cookie(
        &success.token,
        success.expires_at,
        state.config.cookie_secure,
    );
    (
        [
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".parse().expect("valid")),
        ],
        Json(LoginResponse::SignedIn {
            user: success.user.into(),
        }),
    )
        .into_response()
}

/// Signs in with email and password. Sets the session cookie, or asks for a
/// second factor if the user has one.
#[utoipa::path(post, path = "/api/v1/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in (session cookie set), or a second factor is required",
            body = LoginResponse),
        (status = 401, description = "Wrong email or password, or the account is disabled or locked",
            body = ApiError, content_type = "application/problem+json"),
        (status = 429, description = "Too many attempts from this address",
            body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn login(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Response> {
    rate_limit(&state, &ctx)?;
    let outcome = state
        .auth
        .login(&ctx, &body.email, &body.password)
        .await
        .map_err(map_auth_error)?;
    Ok(match outcome {
        LoginOutcome::SignedIn(success) => signed_in(&state, success),
        LoginOutcome::MfaRequired(challenge) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(LoginResponse::MfaRequired {
                challenge: challenge.token,
                expires_at: challenge.expires_at,
                methods: challenge.methods.into_iter().map(Into::into).collect(),
            }),
        )
            .into_response(),
    })
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginMfaRequest {
    /// From the `mfaRequired` response.
    pub challenge: String,
    /// A six-digit code from the authenticator app.
    pub code: Option<String>,
    /// One of the user's recovery codes. Each works once.
    pub recovery_code: Option<String>,
}

/// Completes sign-in with a second factor. Send exactly one of `code` or
/// `recoveryCode`.
#[utoipa::path(post, path = "/api/v1/auth/login/mfa", tag = "auth",
    request_body = LoginMfaRequest,
    responses(
        (status = 200, description = "Signed in; session cookie set", body = LoginResponse),
        (status = 401, description = "Wrong code, or the challenge expired or ran out of attempts",
            body = ApiError, content_type = "application/problem+json"),
        (status = 429, description = "Too many attempts from this address",
            body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn login_mfa(
    State(state): State<AppState>,
    ClientInfo(ctx): ClientInfo,
    Json(body): Json<LoginMfaRequest>,
) -> ApiResult<Response> {
    rate_limit(&state, &ctx)?;
    let factor = match (body.code.as_deref(), body.recovery_code.as_deref()) {
        (Some(code), None) => SecondFactor::Totp(code),
        (None, Some(code)) => SecondFactor::RecoveryCode(code),
        _ => {
            return Err(ApiError::validation()
                .with_field_error("/code", "Send either `code` or `recoveryCode`."));
        }
    };
    let success = state
        .auth
        .complete_mfa(&ctx, &body.challenge, factor)
        .await
        .map_err(map_auth_error)?;
    Ok(signed_in(&state, success))
}

/// Signs out the current session and clears the cookie.
#[utoipa::path(post, path = "/api/v1/auth/logout", tag = "auth",
    responses((status = 204, description = "Signed out")))]
pub async fn logout(
    State(state): State<AppState>,
    user: Option<CurrentUser>,
) -> ApiResult<Response> {
    if let Some(user) = user {
        state
            .auth
            .logout(&user.ctx, user.auth.session_id)
            .await
            .map_err(map_auth_error)?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            session::clear_cookie(state.config.cookie_secure),
        )],
    )
        .into_response())
}

/// The signed-in user.
#[utoipa::path(get, path = "/api/v1/auth/me", tag = "auth",
    responses(
        (status = 200, description = "The signed-in user", body = MeView),
        (status = 401, description = "Not signed in", body = ApiError,
            content_type = "application/problem+json"),
    ))]
pub async fn me(State(state): State<AppState>, authed: Authed) -> ApiResult<Json<MeView>> {
    let mut tx = huxley_db::begin_scoped(&state.db, &huxley_db::OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let active_modules = huxley_modules::states(&mut tx)
        .await
        .map_err(|e| ApiError::internal(&e))?
        .into_iter()
        .filter(|m| m.active)
        .map(|m| m.module.key)
        .collect();
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(Json(MeView {
        active_modules,
        granted_permissions: authed.principal.granted_anywhere(),
        is_owner: authed.principal.is_owner(),
        user: authed.user.auth.user.into(),
    }))
}

/// The signed-in user plus permission hints for the UI.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeView {
    #[serde(flatten)]
    pub user: UserView,
    /// Permissions granted at any organization. For deciding what to show;
    /// the API enforces permissions on every request regardless.
    pub granted_permissions: Vec<&'static str>,
    /// Holds the Owner role at the root organization.
    pub is_owner: bool,
    /// Feature modules that are enabled and released.
    pub active_modules: Vec<&'static str>,
}

/// The signed-in user's active sessions.
#[utoipa::path(get, path = "/api/v1/auth/sessions", tag = "auth",
    responses((status = 200, description = "Active sessions, newest first", body = Vec<SessionView>)))]
pub async fn list_sessions(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<SessionView>>> {
    let sessions = state
        .auth
        .list_sessions(user.auth.user.id)
        .await
        .map_err(map_auth_error)?;
    Ok(Json(
        sessions
            .into_iter()
            .map(|s| SessionView::new(s, user.auth.session_id))
            .collect(),
    ))
}

/// Revokes one of the signed-in user's sessions, e.g. a lost device.
#[utoipa::path(delete, path = "/api/v1/auth/sessions/{id}", tag = "auth",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 204, description = "Session revoked"),
        (status = 404, description = "No such session for this user", body = ApiError,
            content_type = "application/problem+json"),
    ))]
pub async fn revoke_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    state
        .auth
        .revoke_own_session(&user.ctx, user.auth.user.id, id)
        .await
        .map_err(map_auth_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    #[schema(format = Password)]
    pub current_password: String,
    #[schema(format = Password)]
    pub new_password: String,
}

/// Changes the signed-in user's password. Signs out all their other sessions.
#[utoipa::path(post, path = "/api/v1/auth/password", tag = "auth",
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "Password changed; other sessions signed out"),
        (status = 422, description = "Current password wrong or new password too weak",
            body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn change_password(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    state
        .auth
        .change_password(
            &user.ctx,
            user.auth.user.id,
            user.auth.session_id,
            &body.current_password,
            &body.new_password,
        )
        .await
        .map_err(|e| match e {
            AuthError::WrongCurrentPassword => ApiError::validation()
                .with_field_error("/currentPassword", "The current password is incorrect."),
            other => map_auth_error(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}
