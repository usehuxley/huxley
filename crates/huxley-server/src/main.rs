//! `huxley`: the Huxley server binary.
//!
//! One binary, several roles (ADR-001). Small installs run `serve --roles all`;
//! large installs run each role as its own deployment and scale it separately.

mod admin;
mod config;
mod pack_cli;
mod roles;
mod runner;
mod shutdown;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::roles::Role;

#[derive(Debug, Parser)]
#[command(name = "huxley", version, about = "Huxley automation platform server")]
struct Cli {
    /// Path to a TOML config file. Defaults to ./huxley.toml if present.
    #[arg(long, global = true, env = "HUXLEY_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(
    clippy::large_enum_variant,
    reason = "parsed once from the command line"
)]
enum Command {
    /// Run one or more server roles.
    Serve {
        /// Comma-separated roles: api, worker, scheduler, or all.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "all",
            env = "HUXLEY_ROLES"
        )]
        roles: Vec<Role>,
    },
    /// Apply pending database migrations, then exit.
    Migrate,
    /// Print the OpenAPI document as JSON.
    Openapi {
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Administrative tasks: bootstrap, users, audit verification.
    Admin {
        #[command(subcommand)]
        command: admin::AdminCommand,
    },
    /// Make signing keys, build packs, and look inside them.
    Pack {
        #[command(subcommand)]
        command: pack_cli::PackCommand,
    },
    /// Run a script runner (enrol it first).
    Runner {
        #[command(subcommand)]
        command: runner::RunnerCommand,
    },
    /// Probe a running server's /healthz and exit 0 if healthy. Used by
    /// container health checks, since the runtime image has no curl.
    Healthcheck {
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: std::net::SocketAddr,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Healthcheck { addr } => healthcheck(addr),
        Command::Pack { command } => pack_cli::run(command),
        Command::Runner { command } => {
            let telemetry =
                huxley_telemetry::init(&huxley_telemetry::TelemetryConfig::default(), "runner")?;
            let result = runner::run(command).await;
            telemetry.shutdown();
            result
        }
        Command::Admin { command } => {
            let config = Config::load(cli.config.as_deref())?;
            let telemetry = huxley_telemetry::init(&config.telemetry, "admin")?;
            let result = admin::run(&config, command).await;
            telemetry.shutdown();
            result
        }
        Command::Openapi { out } => {
            // Needs no config, database, or telemetry, so CI can run it anywhere.
            let json = huxley_api::openapi::to_pretty_json();
            match out {
                Some(path) => std::fs::write(&path, json + "\n")
                    .with_context(|| format!("failed to write {}", path.display()))?,
                None => println!("{json}"),
            }
            Ok(())
        }
        Command::Migrate => {
            let config = Config::load(cli.config.as_deref())?;
            let telemetry = huxley_telemetry::init(&config.telemetry, "migrate")?;
            let result = run_migrations(&config).await;
            telemetry.shutdown();
            result
        }
        Command::Serve { roles } => {
            let roles = Role::expand(&roles);
            let config = Config::load(cli.config.as_deref())?;
            let telemetry = huxley_telemetry::init(&config.telemetry, &Role::label(&roles))?;
            let result = roles::serve(config, roles).await;
            telemetry.shutdown();
            result
        }
    }
}

async fn run_migrations(config: &Config) -> anyhow::Result<()> {
    // Migrations run as the connecting user, not the RLS-restricted app role.
    let db = huxley_db::connect_owner(config.database()?)?;
    tracing::info!("applying database migrations");
    huxley_db::migrate(&db).await.context("migrations failed")?;
    tracing::info!("database migrations complete");
    Ok(())
}

fn healthcheck(addr: std::net::SocketAddr) -> anyhow::Result<()> {
    use std::io::{Read as _, Write as _};
    use std::time::Duration;

    let timeout = Duration::from_secs(3);
    let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("cannot connect to {addr}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut head = [0u8; 16];
    let n = stream.read(&mut head)?;
    let status_line = String::from_utf8_lossy(&head[..n]);
    anyhow::ensure!(
        status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200"),
        "unhealthy: {status_line}"
    );
    Ok(())
}
