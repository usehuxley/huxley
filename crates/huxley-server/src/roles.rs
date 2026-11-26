//! Server roles and their lifecycle.

use std::collections::BTreeSet;
use std::sync::Arc;
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

/// Starts the schedule triggers (one leader across processes) and the SLA
/// and certification sweepers (safe in several processes).
fn spawn_scheduler(
    tasks: &mut Tasks,
    config: &Config,
    db: &huxley_db::sqlx::PgPool,
    crypto: &huxley_crypto::Crypto,
    storage: &huxley_storage::Storage,
    shutdown: &CancellationToken,
) {
    let engine = huxley_engine::Engine::with_config(db.clone(), crypto.clone(), &config.engine)
        .with_storage(storage.clone())
        .with_public_url(config.auth.public_url.clone())
        .with_portal_url(config.http.api.portal_public_url.clone());
    let stop = shutdown.clone();
    tasks.spawn(async move {
        huxley_triggers::run_scheduler(engine, stop).await;
        Ok(())
    });
    let (sla_db, sla_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        huxley_servicedesk::sla::run_sweeper(sla_db, sla_stop).await;
        Ok(())
    });
    let (wf_db, wf_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        huxley_workforce::run_sweeper(wf_db, wf_stop).await;
        Ok(())
    });
    let (cmdb_db, cmdb_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        huxley_cmdb::warranties::run_sweeper(cmdb_db, cmdb_stop).await;
        Ok(())
    });
    let (contracts_db, contracts_stop) = (db.clone(), shutdown.clone());
    tasks.spawn(async move {
        huxley_contracts::renewals::run_sweeper(contracts_db, contracts_stop).await;
        Ok(())
    });
}

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
        trusted_pack_keys: Arc::new(config.packs.trusted_keys.clone()),
        datasets_console: datasets_console(config).await?,
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
                        .with_public_url(config.auth.public_url.clone())
                        .with_portal_url(config.http.api.portal_public_url.clone())
                        .with_mailer(mailer.clone());
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
                spawn_scheduler(&mut tasks, &config, &db, &crypto, &storage, &shutdown);
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
    // Bind now, so a bad address fails startup at once.
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind the runner gateway on {bind}"))?;
    let engine = huxley_engine::Engine::with_config(db.clone(), crypto.clone(), &config.engine);
    let gateway = huxley_runners::gateway::Gateway::new(db.clone(), crypto.clone(), engine);
    let (db, crypto, hosts, token) = (
        db.clone(),
        crypto.clone(),
        config.runners.hosts.clone(),
        shutdown.clone(),
    );
    tasks.spawn(async move {
        let Some(tls) = gateway_tls(
            &db,
            &crypto,
            &hosts,
            extra.as_deref(),
            &token,
            GATEWAY_RETRY,
        )
        .await?
        else {
            return Ok(());
        };
        tracing::info!(addr = %bind, "runner gateway listening");
        huxley_runners::gateway::serve(gateway, listener, &tls, token)
            .await
            .context("runner gateway failed")
    });
    Ok(())
}

/// How often the runner gateway checks whether the instance is set up yet.
const GATEWAY_RETRY: Duration = Duration::from_secs(5);

/// The gateway's certificate. Its authority is kept under the root
/// organization's key, so before `huxley admin bootstrap` there's none yet:
/// wait for it (the rest of the server runs meanwhile, so the first start of
/// a new instance doesn't fail). `None` if shut down while waiting.
async fn gateway_tls(
    db: &huxley_db::sqlx::PgPool,
    crypto: &huxley_crypto::Crypto,
    hosts: &[String],
    extra_ca: Option<&str>,
    shutdown: &CancellationToken,
    retry: Duration,
) -> anyhow::Result<Option<huxley_runners::gateway::GatewayTls>> {
    let mut waiting = false;
    loop {
        match huxley_runners::gateway::tls(db, crypto, hosts, extra_ca).await {
            Ok(tls) => return Ok(Some(tls)),
            Err(huxley_runners::registry::RegistryError::NoRoot) => {
                if !waiting {
                    tracing::warn!(
                        "runner gateway waiting: the instance isn't set up yet (run `huxley admin bootstrap`)"
                    );
                    waiting = true;
                }
            }
            Err(e) => return Err(anyhow::Error::new(e).context("runner gateway certificate")),
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(None),
            () = tokio::time::sleep(retry) => {}
        }
    }
}

/// The SQL console's pool, if `datasets.console_url` is set; refuses a
/// login that could do more than read datasets.
async fn datasets_console(config: &Config) -> anyhow::Result<Option<huxley_db::sqlx::PgPool>> {
    use secrecy::ExposeSecret as _;
    let Some(url) = &config.datasets.console_url else {
        return Ok(None);
    };
    let pool = huxley_db::sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url.expose_secret())
        .await
        .context("couldn't connect with datasets.console_url")?;
    huxley_datasets::console::check_login(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("datasets.console_url: {e}"))?;
    tracing::info!("datasets SQL console enabled");
    Ok(Some(pool))
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
