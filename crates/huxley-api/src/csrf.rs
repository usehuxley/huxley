//! Cross-site request forgery protection.
//!
//! State-changing API requests (anything but GET/HEAD/OPTIONS) must carry
//! `X-Requested-With: huxley`. Browsers can't add custom headers to
//! cross-site requests without a CORS preflight, and the API never grants
//! cross-origin access, so a forged form or link can't satisfy this. Combined
//! with `SameSite=Lax` session cookies this covers login CSRF too.

use axum::body::Body;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{Method, Request, StatusCode};

use crate::ApiError;

pub const HEADER: &str = "x-requested-with";
pub const VALUE: &str = "huxley";

pub async fn require_header(request: Request<Body>, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let has_header = request
        .headers()
        .get(HEADER)
        .is_some_and(|v| v.as_bytes() == VALUE.as_bytes());
    if safe || has_header {
        next.run(request).await
    } else {
        ApiError::new(StatusCode::FORBIDDEN, "csrf", "Missing anti-forgery header")
            .with_detail(format!(
                "Send `{HEADER}: {VALUE}` with state-changing requests."
            ))
            .into_response()
    }
}
