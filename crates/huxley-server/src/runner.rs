//! `huxley runner`: the script runner side (design doc §6.9, ADR-025).
//! `enroll` trades a single-use token for a client certificate; `run`
//! connects to the gateway with it, sends heartbeats and renews the
//! certificate before it expires. (Running scripts comes next.)

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use huxley_runners::ca;
use huxley_runners::gateway::{self, ClientTls};
use huxley_runners::proto::{HeartbeatRequest, RenewRequest};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Renew when the certificate has less than this left.
const RENEW_BEFORE: Duration = Duration::from_secs(2 * 24 * 3600);
const HEARTBEAT: Duration = Duration::from_secs(30);

#[derive(Debug, Subcommand)]
pub enum RunnerCommand {
    /// Enrol this runner with a token from Settings → Runners.
    Enroll {
        /// Huxley's address, e.g. `https://huxley.example.com`.
        #[arg(long, env = "HUXLEY_SERVER")]
        server: String,
        #[arg(long, env = "HUXLEY_RUNNER_TOKEN")]
        token: String,
        /// The gateway to use, if Huxley doesn't say (`runners.public_url`).
        #[arg(long)]
        gateway: Option<String>,
        #[arg(
            long,
            env = "HUXLEY_RUNNER_STATE",
            default_value = "/var/lib/huxley-runner"
        )]
        state: PathBuf,
    },
    /// Connect to the gateway and keep the runner's certificate fresh.
    Run {
        #[arg(
            long,
            env = "HUXLEY_RUNNER_STATE",
            default_value = "/var/lib/huxley-runner"
        )]
        state: PathBuf,
    },
}

pub async fn run(command: RunnerCommand) -> anyhow::Result<()> {
    match command {
        RunnerCommand::Enroll {
            server,
            token,
            gateway,
            state,
        } => enroll(&server, &token, gateway.as_deref(), &state).await,
        RunnerCommand::Run { state } => {
            let shutdown = CancellationToken::new();
            crate::shutdown::spawn_signal_listener(shutdown.clone());
            serve(&state, shutdown).await
        }
    }
}

fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

async fn enroll(
    server: &str,
    token: &str,
    gateway: Option<&str>,
    state: &Path,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(state)
        .with_context(|| format!("failed to create {}", state.display()))?;
    let (key_pem, csr_pem) = ca::runner_key_and_csr()?;
    let url = format!("{}/api/v1/runners/enroll", server.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(&url)
        .header("x-requested-with", "huxley")
        .json(&json!({ "token": token, "csrPem": csr_pem }))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "enrolment refused ({status}): {}",
            body["title"].as_str().unwrap_or("no details")
        );
    }
    let gateway_url = gateway
        .map(str::to_owned)
        .or_else(|| body["gatewayUrl"].as_str().map(str::to_owned))
        .context("Huxley didn't say where the gateway is: pass --gateway")?;
    write_secret(&state.join("key.pem"), &key_pem)?;
    write_secret(
        &state.join("cert.pem"),
        body["certificatePem"].as_str().unwrap_or_default(),
    )?;
    write_secret(
        &state.join("ca.pem"),
        body["caPem"].as_str().unwrap_or_default(),
    )?;
    write_secret(&state.join("gateway"), &gateway_url)?;
    println!(
        "Enrolled as {} ({}). Start it with `huxley runner run`.",
        body["name"].as_str().unwrap_or_default(),
        body["runnerId"].as_str().unwrap_or_default()
    );
    Ok(())
}

fn read(state: &Path, name: &str) -> anyhow::Result<String> {
    let path = state.join(name);
    std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read {} (enrol first with `huxley runner enroll`)",
            path.display()
        )
    })
}

async fn serve(state: &Path, shutdown: CancellationToken) -> anyhow::Result<()> {
    loop {
        let gateway_url = read(state, "gateway")?;
        let domain = reqwest::Url::parse(gateway_url.trim())
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .context("the gateway address isn't a URL")?;
        let tls = ClientTls {
            ca_pem: read(state, "ca.pem")?,
            cert_pem: read(state, "cert.pem")?,
            key_pem: read(state, "key.pem")?,
            domain,
        };
        let mut client = match gateway::connect(gateway_url.trim(), &tls).await {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(error = %e, "can't reach the gateway; retrying");
                tokio::select! {
                    () = shutdown.cancelled() => return Ok(()),
                    () = tokio::time::sleep(HEARTBEAT) => continue,
                }
            }
        };
        let beat = client
            .heartbeat(HeartbeatRequest {
                languages: vec!["pwsh".into(), "python".into()],
                version: env!("CARGO_PKG_VERSION").into(),
            })
            .await;
        match beat {
            Ok(response) => {
                let response = response.into_inner();
                let left = response.certificate_expires_at
                    - time::OffsetDateTime::now_utc().unix_timestamp();
                if response.certificate_expires_at > 0
                    && u64::try_from(left).unwrap_or(0) < RENEW_BEFORE.as_secs()
                {
                    let (key_pem, csr_pem) = ca::runner_key_and_csr()?;
                    let renewed = client.renew(RenewRequest { csr_pem }).await?.into_inner();
                    write_secret(&state.join("key.pem"), &key_pem)?;
                    write_secret(&state.join("cert.pem"), &renewed.certificate_pem)?;
                    tracing::info!("renewed the runner certificate");
                }
            }
            Err(status) if status.code() == tonic::Code::PermissionDenied => {
                bail!(
                    "the gateway refused this runner: it was revoked, or its certificate replaced"
                );
            }
            Err(status) => tracing::warn!(error = %status, "heartbeat failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = tokio::time::sleep(HEARTBEAT) => {}
        }
    }
}
