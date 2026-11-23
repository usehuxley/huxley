//! Server roles and their lifecycle.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Context as _;
use clap::ValueEnum;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::shutdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Role {
    /// Stateless HTTP tier: REST API, webhooks, MCP.
    Api,
    /// Executes workflow steps from the job queue.
    Worker,
    /// Cron, timers, SLA clocks. Leader-elected.
    Scheduler,
    /// Shorthand for every role in one process.
    All,
}

impl Role {
    /// Expands `all` and removes duplicates.
    pub fn expand(roles: &[Role]) -> BTreeSet<Role> {
        if roles.contains(&Role::All) {
            [Role::Api, Role::Worker, Role::Scheduler].into()
        } else {
            roles.iter().copied().collect()
        }
    }

    /// Label for the `huxley.role` telemetry attribute.
    pub fn label(roles: &BTreeSet<Role>) -> String {
        if roles.len() == 3 {
            return "all".to_owned();
        }
        roles
            .iter()
            .map(|r| match r {
                Role::Api => "api",
                Role::Worker => "worker",
                Role::Scheduler => "scheduler",
                Role::All => "all",
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

type Tasks = JoinSet<anyhow::Result<()>>;

/// Starts the HTTP API and its real-time notification listener.
async fn spawn_api(
    tasks: &mut Tasks,
    config: &Config,
    db: &huxley_db::sqlx::PgPool,
    crypto: &huxley_crypto::Crypto,
    mailer: &huxley_email::Mailer,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.http.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.http.bind))?;
    tracing::info!(addr = %config.http.bind, "api role listening");
    let auth = huxley_auth::AuthService::new(db.clone(), config.auth.clone(), crypto.clone())
        .with_mailer(mailer.clone());
    let storage = huxley_storage::Storage::from_config(&config.storage)?;
    storage
        .check()
        .await
        .with_context(|| format!("object storage ({}) isn't writable", storage.describe()))?;
    tracing::info!(store = storage.describe(), "object storage ready");
    let state =
        huxley_api::AppState::new(db.clone(), auth, crypto.clone(), config.http.api.clone())
            .with_storage(storage);
    let state = huxley_api::AppState {
        runner_gateway_url: config.runners.public_url.clone(),
        ..state
    };
    spawn_runner_gateway(tasks, config, db, crypto, shutdown).await?;
    // Real-time notifications: one LISTEN connection per process.
    let broadcaster = state.notifications.clone();
    let (listen_db, listen_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        broadcaster.run(listen_db, listen_stop).await;
        Ok(())
    });
    // Live run views: likewise, one LISTEN connection per process.
    let run_changes = state.run_changes.clone();
    let (listen_db, listen_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        run_changes.run(listen_db, listen_stop).await;
        Ok(())
    });
    let app =
        huxley_api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
    let token = shutdown.clone();
    tasks.spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(token.cancelled_owned())
            .await
            .context("api server failed")
    });
    Ok(())
}

/// Runs the requested roles until a shutdown signal arrives.
pub async fn serve(config: Config, roles: BTreeSet<Role>) -> anyhow::Result<()> {
    let db_config = config.database()?;
    if config.migrate_on_start {
        tracing::info!("migrate_on_start is set; applying migrations");
        let owner = huxley_db::connect_owner(db_config)?;
        huxley_db::migrate(&owner)
            .await
            .context("migrations failed")?;
        owner.close().await;
    }
    config.auth.validate()?;
    let mailer = huxley_email::Mailer::from_config(&config.email)?;
    if mailer.enabled() && config.auth.public_url.is_none() {
        tracing::warn!("email is configured but auth.public_url isn't: emails need it for links");
    }
    let db = huxley_db::connect(db_config)?;
    let crypto = huxley_crypto::Crypto::new(db.clone(), config.crypto.key_ring()?);
    crypto
        .verify_keys()
        .await
        .context("stored data keys cannot be unwrapped with the configured key-encryption keys")?;

    let shutdown = CancellationToken::new();
    shutdown::spawn_signal_listener(shutdown.clone());

    // Workers spill large step payloads to object storage; the scheduler
    // and event dispatcher start runs with the same engine settings.
    let storage = huxley_storage::Storage::from_config(&config.storage)?;

    let mut tasks = JoinSet::new();
    for role in &roles {
        match role {
            Role::Api => {
                spawn_api(&mut tasks, &config, &db, &crypto, &mailer, &shutdown).await?;
            }
            Role::Worker => {
                // Notification emails. Several workers may run: rows are
                // claimed with SKIP LOCKED.
                let (mail_db, mail_stop) = (db.clone(), shutdown.clone());
                let (mail, public_url) = (mailer.clone(), config.auth.public_url.clone());
                tasks.spawn(async move {
                    huxley_notify::run_email_dispatcher(mail_db, mail, public_url, mail_stop).await;
                    Ok(())
                });
                // Workflow runs: any number of worker processes share the queue.
                let engine =
                    huxley_engine::Engine::with_config(db.clone(), crypto.clone(), &config.engine)
                        .with_storage(storage.clone())
                        .with_public_url(config.auth.public_url.clone());
                let worker_config = huxley_engine::WorkerConfig::from(&config.engine);
                let stop = shutdown.clone();
                let events = (engine.clone(), crypto.clone(), shutdown.clone());
                tasks.spawn(async move {
                    huxley_engine::run_workers(engine, worker_config, stop).await;
                    Ok(())
                });
                // Domain events to event triggers.
                tasks.spawn(async move {
                    huxley_triggers::events::run_event_dispatcher(events.0, events.1, events.2)
                        .await;
                    Ok(())
                });
            }
            Role::Scheduler => {
                // Schedule triggers; one leader at a time across processes.
                let engine =
                    huxley_engine::Engine::with_config(db.clone(), crypto.clone(), &config.engine)
                        .with_storage(storage.clone())
                        .with_public_url(config.auth.public_url.clone());
                let stop = shutdown.clone();
                tasks.spawn(async move {
                    huxley_triggers::run_scheduler(engine, stop).await;
                    Ok(())
                });
            }
            Role::All => unreachable!("expanded before serve"),
        }
    }

    // If any role exits early (e.g. the API fails), stop the others too.
    let first = tasks.join_next().await;
    shutdown.cancel();
    let mut errors: Vec<anyhow::Error> = flatten(first).err().into_iter().collect();

    let grace = Duration::from_secs(config.http.shutdown_grace_secs);
    let drained = tokio::time::timeout(grace, async {
        while let Some(next) = tasks.join_next().await {
            if let Err(e) = flatten(Some(next)) {
                errors.push(e);
            }
        }
    })
    .await;
    if drained.is_err() {
        tracing::warn!(
            grace_secs = grace.as_secs(),
            "shutdown grace period elapsed; aborting remaining tasks"
        );
        tasks.abort_all();
    }

    db.close().await;
    tracing::info!("shutdown complete");
    // Report the first failure; log any others.
    let mut errors = errors.into_iter();
    let first_error = errors.next();
    for e in errors {
        tracing::error!(error = %format!("{e:#}"), "additional role failure");
    }
    first_error.map_or(Ok(()), Err)
}

fn flatten(
    joined: Option<Result<anyhow::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match joined {
        None | Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(e))) => Err(e),
        Some(Err(e)) => Err(anyhow::Error::new(e).context("role task panicked")),
    }
}

/// The runner gateway (ADR-025), when `runners.bind` is set.
async fn spawn_runner_gateway(
    tasks: &mut Tasks,
    config: &Config,
    db: &huxley_db::sqlx::PgPool,
    crypto: &huxley_crypto::Crypto,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let Some(bind) = config.runners.bind else {
        return Ok(());
    };
    let extra = match &config.runners.client_ca_file {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?,
        ),
        None => None,
    };
    let tls = huxley_runners::gateway::tls(db, crypto, &config.runners.hosts, extra.as_deref())
        .await
        .context("runner gateway certificate")?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind the runner gateway on {bind}"))?;
    tracing::info!(addr = %bind, "runner gateway listening");
    let gateway = huxley_runners::gateway::Gateway::new(db.clone(), crypto.clone());
    let token = shutdown.clone();
    tasks.spawn(async move {
        huxley_runners::gateway::serve(gateway, listener, &tls, token)
            .await
            .context("runner gateway failed")
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_expands_to_every_role() {
        let roles = Role::expand(&[Role::All]);
        assert_eq!(roles.len(), 3);
        assert_eq!(Role::label(&roles), "all");
    }

    #[test]
    fn explicit_roles_are_deduplicated() {
        let roles = Role::expand(&[Role::Worker, Role::Api, Role::Worker]);
        assert_eq!(Role::label(&roles), "api,worker");
    }
}
