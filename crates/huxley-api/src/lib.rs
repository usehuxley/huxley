//! Huxley HTTP API.
//!
//! Every route follows the conventions in `docs/api-conventions.md`:
//! RFC 9457 problem details for errors, cursor pagination, `Idempotency-Key`
//! support on unsafe methods, and OpenAPI documentation generated with `utoipa`.

pub mod activity_routes;
pub mod agreement_routes;
mod ai_routes;
pub mod analytics_routes;
mod approval_routes;
mod assistant;
mod assistant_routes;
pub mod audit_routes;
pub mod audit_stream_routes;
pub mod auth_routes;
pub mod authz;
pub mod billing_routes;
pub mod calendar_routes;
pub mod catalog_routes;
pub mod change_routes;
pub mod cmdb_routes;
mod connection_routes;
pub mod context;
pub mod crm_routes;
pub mod csat_routes;
pub mod csrf;
pub mod dashboard_routes;
mod dataset_routes;
pub mod desk_routes;
pub mod entity_routes;
pub mod expense_routes;
mod folder_routes;
mod form_routes;
pub mod health;
mod hook_routes;
pub mod idempotency;
mod integration_routes;
pub mod inventory_routes;
pub mod kb_routes;
pub mod mcp;
pub mod metrics;
pub mod mfa_routes;
pub mod module_routes;
pub mod notification_routes;
pub mod oauth_routes;
pub mod openapi;
pub mod org_routes;
pub mod pack_registry;
mod pack_routes;
pub mod packs;
pub mod passkey_routes;
pub mod portal_access_routes;
pub mod portal_approval_routes;
pub mod portal_catalog_routes;
pub mod portal_dashboard_routes;
pub mod portal_device_routes;
pub mod portal_invoice_routes;
pub mod portal_kb_routes;
pub mod portal_people_routes;
pub mod portal_project_routes;
pub mod portal_quote_routes;
pub mod portal_routes;
pub mod portal_sso_routes;
pub mod portal_ticket_routes;
pub mod problem;
pub mod problem_routes;
pub mod project_routes;
mod public_form_routes;
pub mod purchasing_routes;
pub mod quote_routes;
pub mod rate_limit;
pub mod recommend_routes;
pub mod retention_routes;
mod router;
mod runner_routes;
pub mod sales_order_routes;
pub mod sales_routes;
pub mod scan_routes;
pub mod scim_routes;
pub mod scope_routes;
mod script_routes;
pub mod search_routes;
pub mod security_routes;
pub mod session;
pub mod sso_routes;
pub mod suggestion_routes;
pub mod team_routes;
mod template_routes;
mod timesheet_routes;
pub mod token_routes;
pub mod triage_routes;
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
    /// Take the client address from `X-Forwarded-For` (see `proxy_hops`).
    /// Enable only when every request reaches the API through proxies you
    /// control (Caddy and the web apps in the compose stack).
    pub trust_forwarded_for: bool,
    /// How many proxies in front of Huxley append to `X-Forwarded-For`
    /// (ADR-090): 1 for a reverse proxy or ingress alone, 2 with a CDN
    /// (Cloudflare, Azure Front Door) in front of it. The client is that
    /// many entries from the right. Needs `trust_forwarded_for`.
    pub proxy_hops: usize,
    /// A header your CDN sets to the client's address (`CF-Connecting-IP`,
    /// `X-Azure-ClientIP`, `True-Client-IP`), used before
    /// `X-Forwarded-For` when present. Only when the origin accepts traffic
    /// from the CDN alone, or anyone could set it. Needs
    /// `trust_forwarded_for`.
    pub client_ip_header: Option<String>,
    /// Mark the session cookie `Secure`. Disable only for local HTTP development.
    pub cookie_secure: bool,
    /// Sign-in attempts allowed per client address per minute.
    pub login_attempts_per_minute: u32,
    /// The client portal's public URL (e.g. `https://support.example.com`),
    /// for the sign-in links it emails. Without it, portal links aren't
    /// sent.
    pub portal_public_url: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            trust_forwarded_for: false,
            proxy_hops: 1,
            client_ip_header: None,
            cookie_secure: true,
            login_attempts_per_minute: 20,
            portal_public_url: None,
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
    /// Pack registries to browse (`packs.registries`, ADR-093).
    pub pack_registries: Arc<pack_registry::PackRegistries>,
    /// The datasets SQL console's login (`datasets.console_url`), if set.
    pub datasets_console: Option<PgPool>,
    /// Reporting queries' read replica, if any (else `db`).
    pub read_db: Option<PgPool>,
    /// The upload scanner (`uploads.scan`, ADR-094), to report on; the
    /// scheduler does the scanning.
    pub scanner: Option<Arc<huxley_scan::Scanner>>,
    /// Recent reporting results (ADR-041).
    pub analytics: Arc<huxley_analytics::Cache>,
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
            db.clone(),
            "login",
            config.login_attempts_per_minute,
            Duration::from_secs(60),
        ));
        let (public_db, oauth_db) = (db.clone(), db.clone());
        // Workflows started from the API render callback URLs too.
        let engine = huxley_engine::Engine::new(db.clone(), crypto.clone())
            .with_public_url(auth.config().public_url.clone())
            .with_portal_url(config.portal_public_url.clone());
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
            pack_registries: Arc::new(pack_registry::PackRegistries::new(Vec::new())),
            datasets_console: None,
            read_db: None,
            scanner: None,
            analytics: Arc::new(huxley_analytics::Cache::default()),
            auth,
            crypto,
            config: Arc::new(config),
            login_limiter,
            public_limiter: Arc::new(RateLimiter::new(
                public_db,
                "public_forms",
                PUBLIC_FORMS_PER_MINUTE,
                Duration::from_secs(60),
            )),
            oauth_limiter: Arc::new(RateLimiter::new(
                oauth_db,
                "oauth",
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
