//! Huxley HTTP API.
//!
//! Every route follows the conventions in `docs/api-conventions.md`:
//! RFC 9457 problem details for errors, cursor pagination, `Idempotency-Key`
//! support on unsafe methods, and OpenAPI documentation generated with `utoipa`.

mod ai_routes;
mod approval_routes;
mod assistant;
mod assistant_routes;
pub mod audit_routes;
pub mod auth_routes;
pub mod authz;
pub mod catalog_routes;
pub mod cmdb_routes;
mod connection_routes;
pub mod context;
pub mod crm_routes;
pub mod csrf;
mod dataset_routes;
pub mod desk_routes;
pub mod entity_routes;
mod folder_routes;
mod form_routes;
pub mod health;
mod hook_routes;
pub mod idempotency;
mod integration_routes;
pub mod kb_routes;
pub mod mcp;
pub mod mfa_routes;
pub mod module_routes;
pub mod notification_routes;
pub mod oauth_routes;
pub mod openapi;
pub mod org_routes;
mod pack_routes;
pub mod packs;
pub mod passkey_routes;
pub mod problem;
mod public_form_routes;
pub mod rate_limit;
mod router;
mod runner_routes;
mod script_routes;
pub mod search_routes;
pub mod security_routes;
pub mod session;
pub mod sso_routes;
pub mod team_routes;
mod template_routes;
pub mod token_routes;
pub mod user_routes;
mod variable_routes;
pub mod vault_routes;
mod workflow_routes;
pub mod workforce_routes;

pub use problem::{ApiError, ApiResult};
pub use router::router;

use std::sync::Arc;
use std::time::Duration;

use huxley_auth::AuthService;
use huxley_crypto::Crypto;
use huxley_db::sqlx::PgPool;
use huxley_notify::Broadcaster;
use huxley_sso::SsoService;
use huxley_storage::Storage;
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
    pub sso: SsoService,
    /// Real-time notification fan-out. Run it with [`Broadcaster::run`].
    pub notifications: Broadcaster,
    /// Run ids whose run or steps changed, for live run views. Run it with
    /// [`Broadcaster::run`].
    pub run_changes: Broadcaster,
    /// Object storage for attachments. In-memory until [`AppState::with_storage`].
    pub storage: Storage,
    /// Starts and cancels workflow runs (workers execute them).
    pub engine: huxley_engine::Engine,
    /// Where runners connect (`runners.public_url`), told to them at enrolment.
    pub runner_gateway_url: Option<String>,
    /// Public keys (`packs.trusted_keys`) whose signed packs are trusted.
    pub trusted_pack_keys: Arc<Vec<String>>,
    /// The datasets SQL console's login (`datasets.console_url`), if set.
    pub datasets_console: Option<PgPool>,
    pub config: Arc<ApiConfig>,
    pub login_limiter: Arc<RateLimiter>,
    /// Public form links, per client address.
    pub public_limiter: Arc<RateLimiter>,
    /// OAuth registration, token and revocation requests, per client address.
    pub oauth_limiter: Arc<RateLimiter>,
}

/// Requests to public form links one client address may make a minute.
pub const PUBLIC_FORMS_PER_MINUTE: u32 = 30;
/// OAuth registration, token and revocation requests one client address may
/// make a minute. Clients refresh hourly; registering is once per client.
pub const OAUTH_REQUESTS_PER_MINUTE: u32 = 30;

impl AppState {
    pub fn new(db: PgPool, auth: AuthService, crypto: Crypto, config: ApiConfig) -> Self {
        let login_limiter = Arc::new(RateLimiter::new(
            config.login_attempts_per_minute,
            Duration::from_secs(60),
        ));
        // Workflows started from the API render callback URLs too.
        let engine = huxley_engine::Engine::new(db.clone(), crypto.clone())
            .with_public_url(auth.config().public_url.clone());
        Self {
            db,
            sso: SsoService::new(auth.clone(), crypto.clone())
                .expect("the HTTP client for single sign-on builds"),
            notifications: Broadcaster::new(),
            run_changes: Broadcaster::for_channel(huxley_engine::runs::RUN_CHANGES),
            storage: Storage::memory(),
            engine,
            runner_gateway_url: None,
            trusted_pack_keys: Arc::new(Vec::new()),
            datasets_console: None,
            auth,
            crypto,
            config: Arc::new(config),
            login_limiter,
            public_limiter: Arc::new(RateLimiter::new(
                PUBLIC_FORMS_PER_MINUTE,
                Duration::from_secs(60),
            )),
            oauth_limiter: Arc::new(RateLimiter::new(
                OAUTH_REQUESTS_PER_MINUTE,
                Duration::from_secs(60),
            )),
        }
    }

    /// Uses `storage` for attachments and spilled workflow payloads.
    #[must_use]
    pub fn with_storage(mut self, storage: Storage) -> Self {
        self.engine = self.engine.with_storage(storage.clone());
        self.storage = storage;
        self
    }
}
