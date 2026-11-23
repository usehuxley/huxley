//! `huxley runner`: the script runner side (design doc §6.9, ADR-025).
//! `enroll` trades a single-use token for a client certificate; `run`
//! connects to the gateway with it, sends heartbeats and renews the
//! certificate before it expires, and runs the scripts it's handed in
//! rootless Podman containers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use clap::Subcommand;
use huxley_runners::ca;
use huxley_runners::exec::{Job, Podman};
use huxley_runners::gateway::{self, ClientTls};
use huxley_runners::proto::{HeartbeatRequest, NextRequest, RenewRequest, ReportRequest};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Renew when the certificate has less than this left.
const RENEW_BEFORE: Duration = Duration::from_secs(2 * 24 * 3600);
const HEARTBEAT: Duration = Duration::from_secs(30);
/// How often an idle worker asks for work.
const POLL: Duration = Duration::from_secs(2);

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
    /// Connect to the gateway, keep the certificate fresh, and run scripts.
    Run {
        #[arg(
            long,
            env = "HUXLEY_RUNNER_STATE",
            default_value = "/var/lib/huxley-runner"
        )]
        state: PathBuf,
        #[command(flatten)]
        sandbox: SandboxArgs,
    },
}

/// How scripts run.
#[derive(Debug, clap::Args)]
pub struct SandboxArgs {
    /// Scripts run at once.
    #[arg(long, env = "HUXLEY_RUNNER_CONCURRENCY", default_value_t = 2)]
    concurrency: usize,
    #[arg(long, env = "HUXLEY_RUNNER_PODMAN", default_value = "podman")]
    podman: String,
    #[arg(
        long,
        env = "HUXLEY_RUNNER_PWSH_IMAGE",
        default_value = "mcr.microsoft.com/powershell:7.5-alpine-3.20"
    )]
    pwsh_image: String,
    #[arg(
        long,
        env = "HUXLEY_RUNNER_PYTHON_IMAGE",
        default_value = "docker.io/library/python:3.13-alpine"
    )]
    python_image: String,
    /// Memory per script container.
    #[arg(long, env = "HUXLEY_RUNNER_MEMORY", default_value = "512m")]
    memory: String,
    /// CPUs per script container.
    #[arg(long, env = "HUXLEY_RUNNER_CPUS", default_value = "1")]
    cpus: String,
    /// Pull the script images when starting (for a fresh runner container).
    #[arg(long, env = "HUXLEY_RUNNER_PULL_IMAGES")]
    pull_images: bool,
    /// Give scripts no network at all (not even through the egress proxy).
    #[arg(long, env = "HUXLEY_RUNNER_NO_NETWORK")]
    no_network: bool,
    /// Let scripts reach private addresses through the proxy (link-local
    /// ones such as cloud metadata stay blocked).
    #[arg(long, env = "HUXLEY_RUNNER_EGRESS_ALLOW_PRIVATE")]
    egress_allow_private: bool,
    /// Host names scripts may reach at private addresses, e.g.
    /// `*.corp.example` (repeatable, or comma-separated).
    #[arg(
        long,
        env = "HUXLEY_RUNNER_EGRESS_PRIVATE_HOSTS",
        value_delimiter = ','
    )]
    egress_private_host: Vec<String>,
    /// Host names scripts may never reach (same patterns).
    #[arg(long, env = "HUXLEY_RUNNER_EGRESS_DENIED_HOSTS", value_delimiter = ',')]
    egress_denied_host: Vec<String>,
}

impl SandboxArgs {
    fn podman(&self) -> Podman {
        Podman {
            binary: self.podman.clone(),
            pwsh_image: self.pwsh_image.clone(),
            python_image: self.python_image.clone(),
            memory: self.memory.clone(),
            cpus: self.cpus.clone(),
            egress: (!self.no_network).then(|| {
                std::sync::Arc::new(huxley_engine::EgressConfig {
                    allow_private: self.egress_allow_private,
                    private_hosts: self.egress_private_host.clone(),
                    denied_hosts: self.egress_denied_host.clone(),
                    ..huxley_engine::EgressConfig::default()
                })
            }),
            ..Podman::default()
        }
    }
}

pub async fn run(command: RunnerCommand) -> anyhow::Result<()> {
    match command {
        RunnerCommand::Enroll {
            server,
            token,
            gateway,
            state,
        } => enroll(&server, &token, gateway.as_deref(), &state).await,
        RunnerCommand::Run { state, sandbox } => {
            let shutdown = CancellationToken::new();
            crate::shutdown::spawn_signal_listener(shutdown.clone());
            let podman = sandbox.podman();
            if !podman.available().await {
                bail!("Podman isn't available here (`{}`)", podman.binary);
            }
            if sandbox.pull_images {
                for image in podman.pull().await {
                    tracing::warn!(%image, "couldn't pull a script image");
                }
            }
            let languages = podman.languages().await;
            if languages.is_empty() {
                tracing::warn!(
                    "no script images are present: pull {} and/or {} with podman",
                    podman.pwsh_image,
                    podman.python_image
                );
            }
            let mut workers = tokio::task::JoinSet::new();
            for _ in 0..sandbox.concurrency.max(1) {
                workers.spawn(work(
                    state.clone(),
                    podman.clone(),
                    languages.clone(),
                    shutdown.clone(),
                ));
            }
            let result = serve(&state, &languages, shutdown.clone()).await;
            shutdown.cancel();
            while workers.join_next().await.is_some() {}
            result
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

async fn client(
    state: &Path,
) -> anyhow::Result<huxley_runners::proto::runner_client::RunnerClient<tonic::transport::Channel>> {
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
    Ok(gateway::connect(gateway_url.trim(), &tls).await?)
}

/// One worker: asks for a script, runs it, reports, repeats.
async fn work(state: PathBuf, podman: Podman, languages: Vec<String>, shutdown: CancellationToken) {
    let mut denied = 0;
    while !shutdown.is_cancelled() {
        let mut client = match client(&state).await {
            Ok(client) => client,
            Err(e) => {
                tracing::debug!(error = %e, "worker can't reach the gateway");
                if pause(&shutdown, HEARTBEAT).await {
                    return;
                }
                continue;
            }
        };
        loop {
            let next = client
                .next(NextRequest {
                    languages: languages.clone(),
                })
                .await;
            let task = match next {
                Ok(response) => {
                    denied = 0;
                    response.into_inner().task
                }
                // Renewed meanwhile: reconnect with the new certificate.
                Err(status) if status.code() == tonic::Code::PermissionDenied && denied < 3 => {
                    denied += 1;
                    break;
                }
                Err(status) => {
                    tracing::warn!(error = %status, "couldn't ask for work");
                    if pause(&shutdown, HEARTBEAT).await {
                        return;
                    }
                    break;
                }
            };
            let Some(task) = task else {
                if pause(&shutdown, POLL).await {
                    return;
                }
                continue;
            };
            tracing::info!(task = %task.id, language = %task.language, "running a script");
            let job = Job {
                language: task.language,
                source: task.source,
                inputs: serde_json::from_str(&task.inputs_json).unwrap_or_else(|_| json!({})),
                timeout: Duration::from_secs(u64::try_from(task.timeout_secs).unwrap_or(60)),
                env: task.env.into_iter().collect(),
                redact: task.redact,
            };
            let (sink, lines) = tokio::sync::mpsc::unbounded_channel();
            let forwarding = tokio::spawn(forward_logs(client.clone(), task.id.clone(), lines));
            let outcome = podman.run_with_logs(&job, Some(sink)).await;
            // The last lines go out before the report replaces them all.
            let _ = forwarding.await;
            let report = ReportRequest {
                task_id: task.id.clone(),
                succeeded: outcome.succeeded,
                output_json: outcome.output.map(|v| v.to_string()),
                error: outcome.error,
                exit_code: outcome.exit_code,
                logs: outcome.logs,
            };
            if let Err(status) = client.report(report).await {
                tracing::warn!(task = %task.id, error = %status, "couldn't report the result");
            }
        }
    }
}

/// How often a running script's new output is sent.
const LOG_EVERY: Duration = Duration::from_secs(1);

/// Sends a running script's output to the gateway in batches, until the
/// script is done (the channel closes).
async fn forward_logs(
    mut client: huxley_runners::proto::runner_client::RunnerClient<tonic::transport::Channel>,
    task_id: String,
    mut lines: tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let mut open = true;
    while open {
        let mut text = String::new();
        let deadline = tokio::time::sleep(LOG_EVERY);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.recv() => {
                    let Some(line) = line else {
                        open = false;
                        break;
                    };
                    text.push_str(&line);
                    if text.len() >= 16 * 1024 {
                        break;
                    }
                }
                () = &mut deadline => break,
            }
        }
        if !text.is_empty() {
            let request = huxley_runners::proto::LogRequest {
                task_id: task_id.clone(),
                text,
            };
            if let Err(status) = client.log(request).await {
                tracing::debug!(error = %status, "couldn't send script output");
            }
        }
    }
}

/// Sleeps for `wait`; true if shutting down.
async fn pause(shutdown: &CancellationToken, wait: Duration) -> bool {
    tokio::select! {
        () = shutdown.cancelled() => true,
        () = tokio::time::sleep(wait) => false,
    }
}

async fn serve(
    state: &Path,
    languages: &[String],
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        let mut client = match client(state).await {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(error = %e, "can't reach the gateway; retrying");
                if pause(&shutdown, HEARTBEAT).await {
                    return Ok(());
                }
                continue;
            }
        };
        let beat = client
            .heartbeat(HeartbeatRequest {
                languages: languages.to_vec(),
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
        if pause(&shutdown, HEARTBEAT).await {
            return Ok(());
        }
    }
}
