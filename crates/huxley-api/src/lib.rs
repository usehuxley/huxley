//! Huxley HTTP API.
//!
//! Every route follows the conventions in `docs/api-conventions.md`:
//! RFC 9457 problem details for errors, cursor pagination, `Idempotency-Key`
//! support on unsafe methods, and OpenAPI documentation generated with `utoipa`.

pub mod audit_routes;
pub mod auth_routes;
pub mod authz;
pub mod context;
pub mod csrf;
pub mod health;
pub mod idempotency;
pub mod mfa_routes;
pub mod module_routes;
pub mod openapi;
pub mod org_routes;
pub mod passkey_routes;
pub mod problem;
pub mod rate_limit;
mod router;
pub mod security_routes;
pub mod session;
pub mod user_routes;

pub use problem::{ApiError, ApiResult};
pub use router::router;

use std::sync::Arc;
use std::time::Duration;

use huxley_auth::AuthService;
use huxley_crypto::Crypto;
use huxley_db::sqlx::PgPool;
use serde::Deserialize;

use crate::rate_limit::RateLimiter;

/// HTTP behavior settings (`[http]` in `huxley.toml`, or `HUXLEY_HTTP__*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Trust the rightmost `X-Forwarded-For` entry as the client address. Enable
    /// only when every request reaches the API through a proxy you control
    /// (Caddy and the web apps in the compose stack).
    pub trust_forwarded_for: bool,
    /// Mark the session cookie `Secure`. Disable only for local HTTP development.
    pub cookie_secure: bool,
    /// Sign-in attempts allowed per client address per minute.
    pub login_attempts_per_minute: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            trust_forwarded_for: false,
            cookie_secure: true,
            login_attempts_per_minute: 20,
        }
    }
}

/// Shared state for all handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: PgPool,
    pub auth: AuthService,
    pub crypto: Crypto,
    pub config: Arc<ApiConfig>,
    pub login_limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(db: PgPool, auth: AuthService, crypto: Crypto, config: ApiConfig) -> Self {
        let login_limiter = Arc::new(RateLimiter::new(
            config.login_attempts_per_minute,
            Duration::from_secs(60),
        ));
        Self {
            db,
            auth,
            crypto,
            config: Arc::new(config),
            login_limiter,
        }
    }
}
