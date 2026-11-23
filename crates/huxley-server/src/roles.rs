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
    let db = huxley_db::connect(db_config)?;
    let crypto = huxley_crypto::Crypto::new(db.clone(), config.crypto.key_ring()?);
    crypto
        .verify_keys()
        .await
        .context("stored data keys cannot be unwrapped with the configured key-encryption keys")?;

    let shutdown = CancellationToken::new();
    shutdown::spawn_signal_listener(shutdown.clone());

    let mut tasks = JoinSet::new();
    for role in &roles {
        match role {
            Role::Api => {
                let listener = tokio::net::TcpListener::bind(config.http.bind)
                    .await
                    .with_context(|| format!("failed to bind {}", config.http.bind))?;
                tracing::info!(addr = %config.http.bind, "api role listening");
                let auth = huxley_auth::AuthService::new(db.clone(), config.auth.clone());
                let state = huxley_api::AppState::new(
                    db.clone(),
                    auth,
                    crypto.clone(),
                    config.http.api.clone(),
                );
                let app = huxley_api::router(state)
                    .into_make_service_with_connect_info::<std::net::SocketAddr>();
                let token = shutdown.clone();
                tasks.spawn(async move {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(token.cancelled_owned())
                        .await
                        .context("api server failed")
                });
            }
            Role::Worker => {
                tasks.spawn(placeholder(
                    "worker",
                    "workflow execution arrives in Phase 2",
                    shutdown.clone(),
                ));
            }
            Role::Scheduler => {
                tasks.spawn(placeholder(
                    "scheduler",
                    "timers and cron arrive in Phase 2",
                    shutdown.clone(),
                ));
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

/// Stand-in for roles whose functionality arrives in later phases. Keeps the
/// process model, telemetry, and shutdown paths exercised from day one.
async fn placeholder(
    name: &'static str,
    note: &'static str,
    token: CancellationToken,
) -> anyhow::Result<()> {
    tracing::info!(role = name, note, "role started");
    token.cancelled().await;
    tracing::info!(role = name, "role stopped");
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
