//! Session cookie handling and the authenticated-user extractor.

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use http::request::Parts;
use http::{HeaderValue, header};
use huxley_auth::Authenticated;
use huxley_core::{Actor, RequestContext};
use time::OffsetDateTime;

use crate::context::ClientInfo;
use crate::{ApiError, AppState};

/// The session cookie. The `__Host-` prefix would be stricter but requires
/// HTTPS, which local development over plain HTTP does not have.
pub const COOKIE_NAME: &str = "hx_session";

/// Reads the session token from the `Cookie` header.
pub fn token_from_headers(headers: &http::HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value)
}

/// `Set-Cookie` value that stores the session.
pub fn set_cookie(token: &str, expires_at: OffsetDateTime, secure: bool) -> HeaderValue {
    let max_age = (expires_at - OffsetDateTime::now_utc())
        .whole_seconds()
        .max(0);
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
    ))
    .expect("token and attributes are valid header characters")
}

/// `Set-Cookie` value that deletes the session cookie.
pub fn clear_cookie(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}"
    ))
    .expect("valid header characters")
}

/// The signed-in user. Rejects with 401 when there's no valid session.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub auth: Authenticated,
    /// Request context with the user as actor, for audit records.
    pub ctx: RequestContext,
}

impl CurrentUser {
    /// Authenticates an API token. Only [`crate::authz::Authed`] accepts
    /// tokens: self-service routes (password, MFA, sessions, tokens) take
    /// `CurrentUser` and need a real session.
    pub(crate) async fn from_api_token(
        parts: &mut Parts,
        state: &AppState,
        secret: &str,
    ) -> Result<Self, ApiError> {
        let ClientInfo(ctx) = ClientInfo::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|never| match never {});
        let auth = state
            .auth
            .authenticate_token(secret, ctx.ip)
            .await
            .map_err(|e| ApiError::internal(&e))?
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::UNAUTHORIZED,
                    "invalid-token",
                    "Invalid API token",
                )
                .with_detail("The token is unknown, expired, or revoked.")
            })?;
        let ctx = ctx.with_actor(Actor::User(auth.user.id));
        Ok(Self { auth, ctx })
    }

    async fn resolve(parts: &mut Parts, state: &AppState) -> Result<Option<Self>, ApiError> {
        let Some(token) = token_from_headers(&parts.headers) else {
            return Ok(None);
        };
        let Some(auth) = state
            .auth
            .authenticate(token)
            .await
            .map_err(|e| ApiError::internal(&e))?
        else {
            return Ok(None);
        };
        let ClientInfo(ctx) = ClientInfo::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|never| match never {});
        let ctx = ctx.with_actor(Actor::User(auth.user.id));
        Ok(Some(Self { auth, ctx }))
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        Self::resolve(parts, state)
            .await?
            .ok_or_else(ApiError::unauthorized)
    }
}

/// `Option<CurrentUser>`: `None` when not signed in; errors only on failures.
impl OptionalFromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, ApiError> {
        Self::resolve(parts, state).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut headers = http::HeaderMap::new();
        headers.append(
            header::COOKIE,
            "theme=dark; hx_session=hxs_abc".parse().unwrap(),
        );
        assert_eq!(token_from_headers(&headers), Some("hxs_abc"));

        let mut headers = http::HeaderMap::new();
        headers.append(header::COOKIE, "a=1".parse().unwrap());
        headers.append(header::COOKIE, "hx_session=hxs_xyz".parse().unwrap());
        assert_eq!(token_from_headers(&headers), Some("hxs_xyz"));

        assert_eq!(token_from_headers(&http::HeaderMap::new()), None);
    }

    #[test]
    fn cookie_attributes_are_hardened() {
        let expires = OffsetDateTime::now_utc() + time::Duration::hours(1);
        let value = set_cookie("hxs_abc", expires, true);
        let value = value.to_str().unwrap();
        assert!(value.starts_with("hx_session=hxs_abc; Path=/; Max-Age="));
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Lax"));
        assert!(value.ends_with("; Secure"));
        assert!(
            !set_cookie("t", expires, false)
                .to_str()
                .unwrap()
                .contains("Secure")
        );
    }
}
