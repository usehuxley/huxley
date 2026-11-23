//! `/api/v1/auth/passkeys`: the signed-in user's passkeys.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use huxley_auth::{Ceremony, PasskeyInfo, RegisterPublicKeyCredential};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth_routes::map_auth_error;
use crate::mfa_routes::PasswordConfirmation;
use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(finish_registration))
        .route("/options", post(begin_registration))
        .route("/{id}", patch(rename).delete(remove))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyView {
    pub id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

impl From<PasskeyInfo> for PasskeyView {
    fn from(p: PasskeyInfo) -> Self {
        Self {
            id: p.id,
            name: p.name,
            created_at: p.created_at,
            last_used_at: p.last_used_at,
        }
    }
}

/// The first half of a WebAuthn ceremony.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CeremonyView {
    /// Send back with the browser's answer.
    pub ceremony: Uuid,
    /// Pass `options.publicKey` to `PublicKeyCredential.parseCreationOptionsFromJSON`
    /// (registration) or `parseRequestOptionsFromJSON` (sign-in), then to
    /// `navigator.credentials.create` / `.get`.
    #[schema(value_type = Object)]
    pub options: serde_json::Value,
}

impl From<Ceremony> for CeremonyView {
    fn from(c: Ceremony) -> Self {
        Self {
            ceremony: c.id,
            options: c.options,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FinishRegistrationRequest {
    pub ceremony: Uuid,
    /// A label to tell passkeys apart, e.g. "Work laptop".
    pub name: String,
    /// `credential.toJSON()` from `navigator.credentials.create`.
    #[schema(value_type = Object)]
    pub credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredPasskeyView {
    pub passkey: PasskeyView,
    /// Present when this was the user's first second factor. Shown once.
    pub recovery_codes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenamePasskeyRequest {
    pub name: String,
}

/// The signed-in user's passkeys.
#[utoipa::path(get, path = "/api/v1/auth/passkeys", tag = "auth",
    responses((status = 200, body = Vec<PasskeyView>)))]
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<Vec<PasskeyView>>> {
    let passkeys = state
        .auth
        .list_passkeys(user.auth.user.id)
        .await
        .map_err(map_auth_error)?;
    Ok(Json(passkeys.into_iter().map(Into::into).collect()))
}

/// Starts adding a passkey (re-checks the password).
#[utoipa::path(post, path = "/api/v1/auth/passkeys/options", tag = "auth",
    request_body = PasswordConfirmation,
    responses(
        (status = 200, body = CeremonyView),
        (status = 422, description = "Wrong password", body = ApiError, content_type = "application/problem+json"),
        (status = 503, description = "Passkeys aren't configured", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn begin_registration(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<PasswordConfirmation>,
) -> ApiResult<Json<CeremonyView>> {
    let ceremony = state
        .auth
        .begin_passkey_registration(&user.ctx, &user.auth.user, &body.password)
        .await
        .map_err(map_auth_error)?;
    Ok(Json(ceremony.into()))
}

/// Finishes adding a passkey with the browser's answer.
#[utoipa::path(post, path = "/api/v1/auth/passkeys", tag = "auth",
    request_body = FinishRegistrationRequest,
    responses(
        (status = 201, body = RegisteredPasskeyView),
        (status = 409, description = "Already registered", body = ApiError, content_type = "application/problem+json"),
        (status = 422, description = "Invalid answer, name, or expired request", body = ApiError, content_type = "application/problem+json"),
    ))]
pub async fn finish_registration(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<FinishRegistrationRequest>,
) -> ApiResult<(
    StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    Json<RegisteredPasskeyView>,
)> {
    let registered = state
        .auth
        .finish_passkey_registration(
            &user.ctx,
            user.auth.user.id,
            body.ceremony,
            &body.name,
            &body.credential,
        )
        .await
        .map_err(map_auth_error)?;
    Ok((
        StatusCode::CREATED,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(RegisteredPasskeyView {
            passkey: registered.passkey.into(),
            recovery_codes: registered.recovery_codes,
        }),
    ))
}

/// Renames a passkey.
#[utoipa::path(patch, path = "/api/v1/auth/passkeys/{id}", tag = "auth",
    params(("id" = Uuid, Path, description = "Passkey id")),
    request_body = RenamePasskeyRequest,
    responses((status = 200, body = PasskeyView), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn rename(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<RenamePasskeyRequest>,
) -> ApiResult<Json<PasskeyView>> {
    let passkey = state
        .auth
        .rename_passkey(&user.ctx, user.auth.user.id, id, &body.name)
        .await
        .map_err(map_auth_error)?;
    Ok(Json(passkey.into()))
}

/// Removes a passkey. Removing the last second factor also deletes the
/// recovery codes.
#[utoipa::path(delete, path = "/api/v1/auth/passkeys/{id}", tag = "auth",
    params(("id" = Uuid, Path, description = "Passkey id")),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    state
        .auth
        .delete_passkey(&user.ctx, user.auth.user.id, id)
        .await
        .map_err(map_auth_error)?;
    Ok(StatusCode::NO_CONTENT)
}
