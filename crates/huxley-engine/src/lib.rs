//! The Huxley workflow engine (design doc §6.4, ADR-002, ADR-003, ADR-021).
//!
//! - [`queue`]: the PostgreSQL job queue.
//! - The journal (`step_runs`) records every step attempt; an **advance**
//!   job walks the document against it and makes progress; a **step** job
//!   executes one step with side effects.
//! - [`run_workers`] runs the worker loop: claim, execute, heartbeat.
//!
//! Start runs with [`Engine::start_run`] inside the transaction that wants
//! them; they begin once it commits.

mod action;
mod ai;
pub mod approval;
mod autodoc;
mod calendar;
mod callback;
mod cmdb;
mod crm;
mod csat;
mod dataset;
mod desk;
mod dispatch;
mod egress;
mod email;
mod expenses;
mod http;
mod journal;
mod kb;
mod lookup;
mod notify;
pub mod oauth;
mod pages;
mod payload;
pub mod queue;
mod rate;
pub mod replay;
mod run;
pub mod runs;
pub mod script;
mod script_credentials;
mod step;
mod tickets;
mod timesheets;
mod triage;
mod walk;
mod worker;

use std::sync::Arc;

use huxley_crypto::{Crypto, CryptoError};
use huxley_db::DbError;
use sqlx::PgPool;
use uuid::Uuid;

pub use callback::CallbackRequest;
pub use egress::{Blocked, EgressConfig, allowed_addrs};
pub use journal::{StepStatus, step_key};
pub use payload::{PayloadConfig, SPILLED};
pub use run::{Preview, RunStatus, StartRun, secret_input_names};
pub use walk::MAX_FOREACH_ITEMS;
pub use worker::{EngineConfig, WorkerConfig, run_workers};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// A stored document or value couldn't be read.
    #[error("{0}")]
    Document(String),
    #[error("start a run with either a published version or a draft document")]
    InvalidStart,
    #[error("organization variables: {0}")]
    Variables(#[from] huxley_variables::VariableError),
    /// Object storage (spilled payloads) failed.
    #[error("object storage: {0}")]
    Storage(String),
}

impl From<sqlx::Error> for EngineError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(error))
    }
}

pub type EngineResult<T> = Result<T, EngineError>;

/// The engine. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Engine {
    db: PgPool,
    crypto: Crypto,
    expr: Arc<huxley_expr::Engine>,
    egress: egress::Egress,
    payload_config: PayloadConfig,
    payloads: payload::Payloads,
    /// `auth.public_url`: where callback URLs point.
    public_url: Option<String>,
    /// Sends `core.email` steps' mail.
    mailer: Option<huxley_email::Mailer>,
    /// The client portal's address (`http.portal_public_url`), for links in
    /// mail to contacts when their company has no portal domain.
    portal_url: Option<String>,
}

impl Engine {
    /// An engine with the default settings (public egress only).
    pub fn new(db: PgPool, crypto: Crypto) -> Self {
        Self::with_config(db, crypto, &EngineConfig::default())
    }

    pub fn with_config(db: PgPool, crypto: Crypto, config: &EngineConfig) -> Self {
        Self {
            db,
            crypto,
            expr: Arc::new(huxley_expr::Engine::new()),
            egress: egress::Egress::new(&config.egress),
            payload_config: config.payload.clone(),
            payloads: payload::Payloads::new(None, &config.payload),
            public_url: None,
            mailer: None,
            portal_url: None,
        }
    }

    /// The client portal's address for `org_id`'s contacts: its portal
    /// domain (set there or above), else `http.portal_public_url`.
    pub(crate) async fn portal_base(
        &self,
        conn: &mut sqlx::PgConnection,
        org_id: Uuid,
    ) -> EngineResult<Option<String>> {
        let domain =
            huxley_orgs::settings::value(conn, org_id, huxley_orgs::settings::PORTAL_DOMAIN)
                .await
                .map_err(|e| EngineError::Document(e.to_string()))?;
        Ok(match domain {
            Some(d) if d == "localhost" || d.starts_with("localhost:") => {
                Some(format!("http://{d}"))
            }
            Some(d) => Some(format!("https://{d}")),
            None => self.portal_url.clone(),
        })
    }

    /// Sets the client portal's address (`http.portal_public_url`).
    #[must_use]
    pub fn with_portal_url(mut self, portal_url: Option<String>) -> Self {
        self.portal_url = portal_url;
        self
    }

    /// Sets the address callback URLs point at (`auth.public_url`).
    #[must_use]
    pub fn with_public_url(mut self, public_url: Option<String>) -> Self {
        self.public_url = public_url;
        self
    }

    /// The mail server `core.email` steps send through.
    #[must_use]
    pub fn with_mailer(mut self, mailer: huxley_email::Mailer) -> Self {
        self.mailer = Some(mailer);
        self
    }

    /// Spills large step inputs and outputs to `storage` (design doc §6.4).
    /// Without it, everything stays in the database; an engine that reads
    /// runs other processes wrote needs it to load what they spilled.
    #[must_use]
    pub fn with_storage(mut self, storage: huxley_storage::Storage) -> Self {
        self.payloads = payload::Payloads::new(Some(storage), &self.payload_config);
        self
    }

    /// Reads a spilled step payload (an `input_ref` / `output_ref`).
    pub async fn load_payload(&self, key: &str) -> EngineResult<serde_json::Value> {
        self.payloads.load(key).await
    }

    pub const fn db(&self) -> &PgPool {
        &self.db
    }

    /// The expression engine, for checking documents.
    /// AI calls for organizations, through the egress policy.
    pub fn ai(&self) -> huxley_ai::Ai {
        huxley_ai::Ai::new(
            self.db.clone(),
            self.crypto.clone(),
            self.egress.client.clone(),
        )
    }

    pub fn expr(&self) -> &huxley_expr::Engine {
        &self.expr
    }

    /// Processes one job: dispatches on its kind. Handlers remove or
    /// reschedule the job themselves, in the transaction that records the
    /// result.
    pub async fn process(&self, job: &queue::Job, worker: &str) -> EngineResult<()> {
        match job.kind.as_str() {
            queue::kind::ADVANCE => self.advance(job, worker).await,
            queue::kind::STEP => self.execute_step(job, worker).await,
            queue::kind::DEADLINE => self.deadline(job, worker).await,
            other => Err(EngineError::Document(format!("unknown job kind {other:?}"))),
        }
    }

    /// Claims and processes due jobs one at a time until none are left (or
    /// `limit` were processed). Returns how many ran. For tests and tools;
    /// servers use [`run_workers`].
    pub async fn drain(&self, worker: &str, limit: usize) -> EngineResult<usize> {
        let mut done = 0;
        while done < limit {
            let Some(job) =
                worker::claim_one(self, worker, std::time::Duration::from_secs(60)).await?
            else {
                break;
            };
            if let Err(error) = self.process(&job, worker).await {
                worker::record_failure(self, &job, &error).await?;
            }
            done += 1;
        }
        Ok(done)
    }
}
