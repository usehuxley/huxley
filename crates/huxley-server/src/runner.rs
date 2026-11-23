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
use huxley_runners::sandbox::Sandbox;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Renew when the certificate has less than this left.
const RENEW_BEFORE: Duration = Duration::from_secs(2 * 24 * 3600);
const HEARTBEAT: Duration = Duration::from_secs(30);
/// How often an idle worker asks for work.
const POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Subcommand)]
#[allow(
    clippy::large_enum_variant,
    reason = "parsed once from the command line"
)]
pub enum RunnerCommand {
    /// Enrol this runner with a token from Settings → Runners.
    Enroll {
        /// Huxley's address, e.g. `https://huxley.example.com`.
        #[arg(long, env = "HUXLEY_SERVER")]
        server: String,
        #[arg(long, env = "HUXLEY_RUNNER_TOKEN", hide_env_values = true)]
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
    /// The egress proxy for the Kubernetes sandbox: script pods' only way
    /// out (`deploy/kubernetes` runs it).
    EgressProxy {
        #[arg(
            long,
            env = "HUXLEY_RUNNER_PROXY_LISTEN",
            default_value = "0.0.0.0:3128"
        )]
        listen: std::net::SocketAddr,
        #[command(flatten)]
        egress: EgressArgs,
    },
    /// Connect to the gateway, keep the certificate fresh, and run scripts.
    Run {
        #[arg(
            long,
            env = "HUXLEY_RUNNER_STATE",
            default_value = "/var/lib/huxley-runner"
        )]
        state: PathBuf,
        /// With `--token`: enrol on first start (e.g. in Kubernetes, from
        /// a Secret). Ignored once enrolled.
        #[arg(long, env = "HUXLEY_SERVER")]
        server: Option<String>,
        #[arg(long, env = "HUXLEY_RUNNER_TOKEN", hide_env_values = true)]
        token: Option<String>,
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
    /// Where scripts run.
    #[arg(
        long,
        env = "HUXLEY_RUNNER_SANDBOX",
        value_enum,
        default_value = "podman"
    )]
    sandbox: SandboxKind,
    #[command(flatten)]
    kubernetes: KubernetesArgs,
    #[command(flatten)]
    egress: EgressArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum SandboxKind {
    /// A rootless Podman container per script, on this host.
    Podman,
    /// A Kubernetes Job per script (deploy/kubernetes sets the cluster up).
    Kubernetes,
}

/// The Kubernetes Jobs sandbox (`--sandbox kubernetes`).
#[derive(Debug, clap::Args)]
pub struct KubernetesArgs {
    /// The namespace scripts run in. Default: the runner's own namespace
    /// in a cluster, else `huxley-scripts`.
    #[arg(long = "k8s-namespace", env = "HUXLEY_RUNNER_K8S_NAMESPACE")]
    namespace: Option<String>,
    /// A runtime class for script pods, e.g. `gvisor`.
    #[arg(long = "k8s-runtime-class", env = "HUXLEY_RUNNER_K8S_RUNTIME_CLASS")]
    runtime_class: Option<String>,
    /// The network policy that confines script pods; the runner won't
    /// start without it.
    #[arg(
        long = "k8s-network-policy",
        env = "HUXLEY_RUNNER_K8S_NETWORK_POLICY",
        default_value = "huxley-scripts"
    )]
    network_policy: String,
    /// The egress proxy's Service, in the same namespace.
    #[arg(
        long = "k8s-egress-proxy",
        env = "HUXLEY_RUNNER_K8S_EGRESS_PROXY",
        default_value = "huxley-egress-proxy"
    )]
    egress_proxy: String,
}

/// The egress policy: for the Podman sandbox's proxy, and for
/// `huxley runner egress-proxy`.
#[derive(Debug, Clone, clap::Args)]
pub struct EgressArgs {
    /// Let scripts reach private addresses through the proxy (link-local
    /// ones such as cloud metadata stay blocked).
    #[arg(
        long = "egress-allow-private",
        env = "HUXLEY_RUNNER_EGRESS_ALLOW_PRIVATE"
    )]
    allow_private: bool,
    /// Host names scripts may reach at private addresses, e.g.
    /// `*.corp.example` (repeatable, or comma-separated).
    #[arg(
        long = "egress-private-host",
        env = "HUXLEY_RUNNER_EGRESS_PRIVATE_HOSTS",
        value_delimiter = ','
    )]
    private_host: Vec<String>,
    /// Host names scripts may never reach (same patterns).
    #[arg(
        long = "egress-denied-host",
        env = "HUXLEY_RUNNER_EGRESS_DENIED_HOSTS",
        value_delimiter = ','
    )]
    denied_host: Vec<String>,
}

impl EgressArgs {
    fn policy(&self) -> std::sync::Arc<huxley_engine::EgressConfig> {
        std::sync::Arc::new(huxley_engine::EgressConfig {
            allow_private: self.allow_private,
            private_hosts: self.private_host.clone(),
            denied_hosts: self.denied_host.clone(),
            ..huxley_engine::EgressConfig::default()
        })
    }
}

/// Podman sizes (`512m`, `2g`) as Kubernetes quantities (`512Mi`, `2Gi`).
fn kube_memory(memory: &str) -> String {
    let lower = memory.trim().to_ascii_lowercase();
    for (suffix, unit) in [("k", "Ki"), ("m", "Mi"), ("g", "Gi")] {
        if let Some(n) = lower.strip_suffix(suffix)
            && n.chars().all(|c| c.is_ascii_digit())
            && !n.is_empty()
        {
            return format!("{n}{unit}");
        }
    }
    memory.to_owned()
}

impl SandboxArgs {
    async fn kubernetes(&self) -> anyhow::Result<huxley_runners::kubernetes::Kubernetes> {
        huxley_runners::install_crypto_provider();
        let client = kube::Client::try_default()
            .await
            .context("couldn't connect to Kubernetes (in-cluster or kubeconfig)")?;
        let namespace = self.kubernetes.namespace.clone().unwrap_or_else(|| {
            std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
                .map_or_else(|_| "huxley-scripts".to_owned(), |n| n.trim().to_owned())
        });
        let proxy_url = if self.no_network {
            None
        } else {
            let services: kube::Api<k8s_openapi::api::core::v1::Service> =
                kube::Api::namespaced(client.clone(), &namespace);
            let name = &self.kubernetes.egress_proxy;
            let service = services
                .get(name)
                .await
                .with_context(|| format!("no egress proxy Service {name} in {namespace}"))?;
            let spec = service.spec.unwrap_or_default();
            let ip = spec
                .cluster_ip
                .filter(|ip| ip != "None")
                .with_context(|| format!("Service {name} has no cluster IP"))?;
            let port = spec
                .ports
                .and_then(|p| p.first().map(|p| p.port))
                .unwrap_or(3128);
            // An address, not a name: script pods get no DNS.
            Some(format!("http://{ip}:{port}"))
        };
        let kubernetes = huxley_runners::kubernetes::Kubernetes {
            client,
            namespace,
            pwsh_image: self.pwsh_image.clone(),
            python_image: self.python_image.clone(),
            memory: kube_memory(&self.memory),
            cpus: self.cpus.clone(),
            runtime_class: self.kubernetes.runtime_class.clone(),
            proxy_url,
        };
        kubernetes
            .check_policy(&self.kubernetes.network_policy)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(kubernetes)
    }

    fn podman(&self) -> Podman {
        Podman {
            binary: self.podman.clone(),
            pwsh_image: self.pwsh_image.clone(),
            python_image: self.python_image.clone(),
            memory: self.memory.clone(),
            cpus: self.cpus.clone(),
            egress: (!self.no_network).then(|| self.egress.policy()),
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
        RunnerCommand::EgressProxy { listen, egress } => egress_proxy(listen, &egress).await,
        RunnerCommand::Run {
            state,
            server,
            token,
            sandbox,
        } => {
            if !state.join("cert.pem").exists()
                && let (Some(server), Some(token)) = (&server, &token)
            {
                enroll(server, token, None, &state).await?;
            }
            let shutdown = CancellationToken::new();
            crate::shutdown::spawn_signal_listener(shutdown.clone());
            let (runner, languages) = prepare(&sandbox).await?;
            let mut workers = tokio::task::JoinSet::new();
            for _ in 0..sandbox.concurrency.max(1) {
                workers.spawn(work(
                    state.clone(),
                    runner.clone(),
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

/// The sandbox, checked, and the languages it can run.
async fn prepare(args: &SandboxArgs) -> anyhow::Result<(Sandbox, Vec<String>)> {
    if args.sandbox == SandboxKind::Kubernetes {
        let kubernetes = args.kubernetes().await?;
        tracing::info!(namespace = %kubernetes.namespace, "running scripts as Kubernetes Jobs");
        return Ok((
            Sandbox::Kubernetes(kubernetes),
            vec!["pwsh".into(), "python".into()],
        ));
    }
    let podman = args.podman();
    if !podman.available().await {
        bail!("Podman isn't available here (`{}`)", podman.binary);
    }
    if args.pull_images {
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
    Ok((Sandbox::Podman(podman), languages))
}

/// `huxley runner egress-proxy`: the Kubernetes sandbox's egress proxy.
async fn egress_proxy(listen: std::net::SocketAddr, egress: &EgressArgs) -> anyhow::Result<()> {
    let shutdown = CancellationToken::new();
    crate::shutdown::spawn_signal_listener(shutdown.clone());
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("failed to listen on {listen}"))?;
    tracing::info!(addr = %listen, "egress proxy listening");
    huxley_runners::proxy::serve_tcp(listener, egress.policy(), shutdown).await;
    Ok(())
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
async fn work(
    state: PathBuf,
    sandbox: Sandbox,
    languages: Vec<String>,
    shutdown: CancellationToken,
) {
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
            let outcome = sandbox.run_with_logs(&job, Some(sink)).await;
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

#[cfg(test)]
mod tests {
    use super::kube_memory;

    #[test]
    fn podman_sizes_become_kubernetes_quantities() {
        assert_eq!(kube_memory("512m"), "512Mi");
        assert_eq!(kube_memory("2G"), "2Gi");
        assert_eq!(kube_memory("768Mi"), "768Mi");
        assert_eq!(kube_memory("1000000"), "1000000");
    }
}
