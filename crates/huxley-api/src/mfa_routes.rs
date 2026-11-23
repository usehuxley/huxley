//! `/api/v1/auth/mfa`: the signed-in user's own second factors.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth_routes::map_auth_error;
use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(status))
        .route("/totp", post(begin_totp))
        .route("/totp/confirm", post(confirm_totp))
        .route("/totp/disable", post(disable_totp))
        .route("/recovery-codes", post(regenerate_recovery_codes))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaStatusView {
    /// An authenticator app is set up and required at sign-in.
    pub totp_enabled: bool,
    /// Unused recovery codes left.
    pub recovery_codes_remaining: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordConfirmation {
    /// The user's current password, re-checked before the change.
    #[schema(format = Password)]
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TotpEnrollmentView {
    /// Base32 secret, for entering into the app by hand.
    pub secret: String,
    /// `otpauth://` URI to show as a QR code.
    pub otpauth_uri: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmTotpRequest {
    /// A current six-digit code from the app.
    pub code: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCodesView {
    /// Shown only now. Each code signs in once if the authenticator is lost.
    pub recovery_codes: Vec<String>,
}

fn no_store<T: Serialize>(body: T) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

/// The signed-in user's MFA setup.
#[utoipa::path(get, path = "/api/v1/auth/mfa", tag = "auth",
    responses((status = 200, body = MfaStatusView)))]
pub async fn status(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<MfaStatusView>> {
    let status = state
        .auth
        .mfa_status(user.auth.user.id)
        .await
        .map_err(map_auth_error)?;
    Ok(Json(MfaStatusView {
        totp_enabled: status.totp_enabled,
        recovery_codes_remaining: status.recovery_codes_remaining,
    }))
}

/// Starts setting up an authenticator app. Returns the secret to scan; the
/// app isn't required at sign-in until `/totp/confirm` succeeds.
#[utoipa::path(post, path = "/api/v1/auth/mfa/totp", tag = "auth",
    request_body = PasswordConfirmation,
    responses(
        (status = 200, body = TotpEnrollmentView),
        (status = 409, description = "Already set up", body = ApiError, content_type = "application/problem+json"),
        (status = 422, description = "Wrong password", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn begin_totp(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<Response> {
    let enrollment = state
        .auth
        .begin_totp(
            &user.ctx,
            user.auth.user.id,
            &user.auth.user.email,
            &body.password,
        )
        .await
        .map_err(map_auth_error)?;
    Ok(no_store(TotpEnrollmentView {
        secret: enrollment.secret,
        otpauth_uri: enrollment.otpauth_uri,
    }))
}

/// Finishes authenticator setup with a code from the app. Returns recovery
/// codes (shown once) and signs out the user's other sessions.
#[utoipa::path(post, path = "/api/v1/auth/mfa/totp/confirm", tag = "auth",
    request_body = ConfirmTotpRequest,
    responses(
        (status = 200, body = RecoveryCodesView),
        (status = 401, description = "Wrong code", body = ApiError, content_type = "application/problem+json"),
        (status = 409, description = "No setup in progress", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn confirm_totp(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ConfirmTotpRequest>,
) -> ApiResult<Response> {
    let codes = state
        .auth
        .confirm_totp(
            &user.ctx,
            user.auth.user.id,
            user.auth.session_id,
            &body.code,
        )
        .await
        .map_err(map_auth_error)?;
    Ok(no_store(RecoveryCodesView {
        recovery_codes: codes,
    }))
}

/// Turns off the authenticator app and deletes recovery codes.
#[utoipa::path(post, path = "/api/v1/auth/mfa/totp/disable", tag = "auth",
    request_body = PasswordConfirmation,
    responses(
        (status = 204, description = "Turned off"),
        (status = 409, description = "Not set up", body = ApiError, content_type = "application/problem+json"),
        (status = 422, description = "Wrong password", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn disable_totp(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<StatusCode> {
    state
        .auth
        .disable_totp(&user.ctx, user.auth.user.id, &body.password)
        .await
        .map_err(map_auth_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces the recovery codes. The old ones stop working.
#[utoipa::path(post, path = "/api/v1/auth/mfa/recovery-codes", tag = "auth",
    request_body = PasswordConfirmation,
    responses(
        (status = 200, body = RecoveryCodesView),
        (status = 409, description = "MFA not set up", body = ApiError, content_type = "application/problem+json"),
        (status = 422, description = "Wrong password", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<Response> {
    let codes = state
        .auth
        .regenerate_recovery_codes(&user.ctx, user.auth.user.id, &body.password)
        .await
        .map_err(map_auth_error)?;
    Ok(no_store(RecoveryCodesView {
        recovery_codes: codes,
    }))
}
