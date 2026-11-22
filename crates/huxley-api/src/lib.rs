//! Huxley HTTP API.
//!
//! Every route follows the conventions in `docs/api-conventions.md`:
//! RFC 9457 problem details for errors, cursor pagination, `Idempotency-Key`
//! support on unsafe methods, and OpenAPI documentation generated with `utoipa`.

pub mod health;
pub mod idempotency;
pub mod openapi;
pub mod problem;
mod router;

pub use problem::{ApiError, ApiResult};
pub use router::router;

use huxley_db::sqlx::PgPool;

/// Shared state for all handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: PgPool,
}
