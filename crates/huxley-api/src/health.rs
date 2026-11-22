//! Liveness and readiness probes.
//!
//! - `/healthz`: the process is up. Never touches dependencies, so a database
//!   outage does not get healthy API pods killed and restarted.
//! - `/readyz`: dependencies are reachable, so the pod should receive traffic.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, ApiResult, AppState};

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthStatus {
    /// Always `ok` when the response is 200.
    pub status: &'static str,
    pub version: &'static str,
}

const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// Liveness probe.
#[utoipa::path(get, path = "/healthz", tag = "system",
    responses((status = 200, description = "Process is alive", body = HealthStatus)))]
pub async fn healthz() -> Json<HealthStatus> {
    Json(HealthStatus {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Readiness probe. Checks the database.
#[utoipa::path(get, path = "/readyz", tag = "system",
    responses(
        (status = 200, description = "Ready to serve traffic", body = HealthStatus),
        (status = 503, description = "A dependency is unavailable", body = ApiError,
            content_type = "application/problem+json"),
    ))]
pub async fn readyz(State(state): State<AppState>) -> ApiResult<Json<HealthStatus>> {
    match tokio::time::timeout(READY_TIMEOUT, huxley_db::ping(&state.db)).await {
        Ok(Ok(())) => Ok(Json(HealthStatus {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
        })),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readiness check failed: database");
            Err(ApiError::unavailable("Database is unreachable."))
        }
        Err(_) => {
            tracing::warn!("readiness check timed out: database");
            Err(ApiError::unavailable("Database did not respond in time."))
        }
    }
}
