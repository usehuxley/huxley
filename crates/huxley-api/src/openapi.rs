//! OpenAPI document.
//!
//! Served at `/api/v1/openapi.json` and exported with `huxley openapi`, which
//! is how `packages/api-client` generates its TypeScript types.

use axum::Json;
use utoipa::OpenApi;

use crate::auth_routes::{self, ChangePasswordRequest, LoginRequest, SessionView, UserView};
use crate::health::{self, HealthStatus};
use crate::problem::{ApiError, FieldError};

#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "Huxley API",
        description = "Huxley platform API. See https://docs.usehuxley.com for conventions.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(
        health::healthz,
        health::readyz,
        auth_routes::login,
        auth_routes::logout,
        auth_routes::me,
        auth_routes::change_password,
        auth_routes::list_sessions,
        auth_routes::revoke_session,
    ),
    components(schemas(HealthStatus, ApiError, FieldError, LoginRequest, ChangePasswordRequest, UserView, SessionView)),
    tags(
        (name = "system", description = "Health and platform metadata"),
        (name = "auth", description = "Sign-in, sessions, and the current user"),
    ),
)]
pub struct ApiDoc;

/// Serves the OpenAPI document.
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// The OpenAPI document as pretty-printed JSON.
pub fn to_pretty_json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI document must serialize")
}
