//! RFC 9457 problem details (`application/problem+json`).
//!
//! Every error response from the API uses this shape. `type` is a stable URI
//! per error kind that clients can branch on. `title` is short and stable,
//! `detail` is human-readable and specific to this occurrence.

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use serde::Serialize;
use utoipa::ToSchema;

/// Base URI for problem types. Each kind is documented under this path.
pub const PROBLEM_BASE: &str = "https://usehuxley.io/problems/";

pub type ApiResult<T> = Result<T, ApiError>;

/// A field-level validation error.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FieldError {
    /// JSON Pointer to the invalid field, e.g. `/name`.
    pub pointer: String,
    pub message: String,
}

/// An RFC 9457 problem details response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiError {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[schema(value_type = u16)]
    #[serde(serialize_with = "serialize_status")]
    pub status: StatusCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // signature required by serde
fn serialize_status<S: serde::Serializer>(status: &StatusCode, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u16(status.as_u16())
}

impl ApiError {
    pub fn new(status: StatusCode, slug: &str, title: impl Into<String>) -> Self {
        Self {
            kind: format!("{PROBLEM_BASE}{slug}"),
            title: title.into(),
            status,
            detail: None,
            errors: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The most specific message: a field's error, else the detail, else
    /// the title.
    pub fn summary(&self) -> String {
        self.errors
            .first()
            .map(|e| e.message.clone())
            .or_else(|| self.detail.clone())
            .unwrap_or_else(|| self.title.clone())
    }

    #[must_use]
    pub fn with_field_error(
        mut self,
        pointer: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.errors.push(FieldError {
            pointer: pointer.into(),
            message: message.into(),
        });
        self
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad-request", "Bad request").with_detail(detail)
    }

    pub fn validation() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "Validation failed",
        )
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication required",
        )
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "Permission denied")
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", "Resource not found")
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", "Conflict").with_detail(detail)
    }

    /// A file the malware scan withholds (ADR-094): quarantined (403), or
    /// not scanned clean yet (409, try again shortly).
    pub fn withheld(scan: &huxley_db::scans::Status) -> Self {
        match scan {
            huxley_db::scans::Status::Infected(_) => Self::new(
                StatusCode::FORBIDDEN,
                "file-quarantined",
                "File quarantined",
            )
            .with_detail(
                "The malware scan found a threat in this file, so it can't be downloaded.",
            ),
            huxley_db::scans::Status::Failed(_) => Self::new(
                StatusCode::CONFLICT,
                "file-not-scanned",
                "File not scanned",
            )
            .with_detail(
                "The malware scan couldn't check this file. An administrator can scan it again.",
            ),
            _ => Self::new(StatusCode::CONFLICT, "file-scanning", "File being scanned")
                .with_detail(
                    "This file is still being checked for malware. Try again in a moment.",
                ),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Service unavailable",
        )
        .with_detail(detail)
    }

    /// An unexpected error. The cause is logged but never returned to the client.
    pub fn internal(cause: &dyn std::fmt::Display) -> Self {
        tracing::error!(error = %cause, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let mut response = (status, Json(self)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

impl From<huxley_core::CursorError> for ApiError {
    fn from(_: huxley_core::CursorError) -> Self {
        Self::bad_request("The pagination cursor is invalid or expired.")
    }
}
