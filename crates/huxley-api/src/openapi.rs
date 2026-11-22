//! OpenAPI document.
//!
//! Served at `/api/v1/openapi.json` and exported with `huxley openapi`, which
//! is how `packages/api-client` generates its TypeScript types.

use axum::Json;
use utoipa::OpenApi;

use crate::health::{self, HealthStatus};
use crate::problem::{ApiError, FieldError};

#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "Huxley API",
        description = "Huxley platform API. See https://docs.usehuxley.com for conventions.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(health::healthz, health::readyz),
    components(schemas(HealthStatus, ApiError, FieldError)),
    tags((name = "system", description = "Health and platform metadata")),
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
