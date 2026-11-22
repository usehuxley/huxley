//! `/api/v1/auth`: sign in, sign out, current user, and session management.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use huxley_auth::{AuthError, Session, User, UserStatus};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::context::ClientInfo;
use crate::session::{self, CurrentUser};
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
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

fn map_auth_error(error: AuthError) -> ApiError {
    match error {
        AuthError::InvalidCredentials => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid-credentials",
            "Invalid email or password",
        ),
        AuthError::SessionNotFound => ApiError::not_found(),
        AuthError::WrongCurrentPassword => ApiError::validation()
            .with_field_error("/currentPassword", "The current password is incorrect."),
        AuthError::WeakPassword(e) => {
            ApiError::validation().with_field_error("/newPassword", e.to_string())
        }
        other => ApiError::internal(&other),
    }
}

/// Signs in with email and password. Sets the session cookie.
#[utoipa::path(post, path = "/api/v1/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in; session cookie set", body = UserView),
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

    let success = state
        .auth
        .login(&ctx, &body.email, &body.password)
        .await
        .map_err(map_auth_error)?;

    let cookie = session::set_cookie(
        &success.token,
        success.expires_at,
        state.config.cookie_secure,
    );
    Ok((
        [
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".parse().expect("valid")),
        ],
        Json(UserView::from(success.user)),
    )
        .into_response())
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
        (status = 200, description = "The signed-in user", body = UserView),
        (status = 401, description = "Not signed in", body = ApiError,
            content_type = "application/problem+json"),
    ))]
pub async fn me(user: CurrentUser) -> Json<UserView> {
    Json(user.auth.user.into())
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
        .map_err(map_auth_error)?;
    Ok(StatusCode::NO_CONTENT)
}
