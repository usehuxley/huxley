//! Running a script in a sandbox (design doc §6.9). The default sandbox is
//! a rootless Podman container: no network, a read-only root filesystem,
//! no capabilities, an unprivileged user, and CPU, memory, process and
//! time limits. The script never runs on the runner's host directly.
//!
//! Inside the container the script gets its parameters (as PowerShell
//! parameters, or Python's `inputs` dict) and its result is what it
//! returns: PowerShell's pipeline output, or Python's `result` variable.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use huxley_engine::EgressConfig;
use std::process::Stdio;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use serde_json::Value as Json;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;

/// Logs kept from a run (stdout and stderr together).
const MAX_LOGS: usize = 64 * 1024;

/// The scripts to run, in the container, around the library script.
const PWSH_WRAPPER: &str = r"$ErrorActionPreference = 'Stop'
if (Test-Path /huxley/net/proxy.sock) {
    Add-Type -TypeDefinition (Get-Content -Raw /huxley/in/relay.cs)
    [HuxleyRelay]::Start('/huxley/net/proxy.sock', 3128)
}
$params = Get-Content -Raw /huxley/in/inputs.json | ConvertFrom-Json -AsHashtable
$result = @(& /huxley/in/script.ps1 @params)
$value = if ($result.Count -eq 0) { $null } elseif ($result.Count -eq 1) { $result[0] } else { $result }
ConvertTo-Json -InputObject $value -Depth 32 -Compress | Set-Content -NoNewline /huxley/out/result.json
";

const PYTHON_WRAPPER: &str = r#"import json, os, runpy, socket, threading

def _relay(path, port):
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", port))
    server.listen(64)

    def pump(src, dst):
        try:
            while True:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except OSError:
            pass
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass

    def accept():
        while True:
            client, _ = server.accept()
            upstream = socket.socket(socket.AF_UNIX)
            try:
                upstream.connect(path)
            except OSError:
                client.close()
                continue
            threading.Thread(target=pump, args=(client, upstream), daemon=True).start()
            threading.Thread(target=pump, args=(upstream, client), daemon=True).start()

    threading.Thread(target=accept, daemon=True).start()

if os.path.exists("/huxley/net/proxy.sock"):
    _relay("/huxley/net/proxy.sock", 3128)
with open("/huxley/in/inputs.json") as f:
    inputs = json.load(f)
scope = runpy.run_path("/huxley/in/script.py", init_globals={"inputs": inputs}, run_name="__main__")
with open("/huxley/out/result.json", "w") as f:
    json.dump(scope.get("result"), f, default=str)
"#;

/// The PowerShell side of the egress relay: 127.0.0.1:3128 to the proxy's
/// socket (Python's is in its wrapper).
const PWSH_RELAY: &str = r"using System.Net;
using System.Net.Sockets;
using System.Threading.Tasks;

public static class HuxleyRelay {
    public static void Start(string path, int port) {
        var listener = new TcpListener(IPAddress.Loopback, port);
        listener.Start();
        Task.Run(async () => {
            while (true) {
                var client = await listener.AcceptTcpClientAsync();
                _ = Handle(client, path);
            }
        });
    }

    static async Task Handle(TcpClient client, string path) {
        using (client)
        using (var upstream = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified)) {
            try {
                await upstream.ConnectAsync(new UnixDomainSocketEndPoint(path));
            } catch (SocketException) {
                return;
            }
            var a = Pump(client.Client, upstream);
            var b = Pump(upstream, client.Client);
            await Task.WhenAll(a, b);
        }
    }

    static async Task Pump(Socket src, Socket dst) {
        var buffer = new byte[65536];
        try {
            while (true) {
                int n = await src.ReceiveAsync(buffer, SocketFlags.None);
                if (n == 0) break;
                await dst.SendAsync(new System.ArraySegment<byte>(buffer, 0, n), SocketFlags.None);
            }
        } catch (SocketException) {
        } catch (System.ObjectDisposedException) {
        }
        try { dst.Shutdown(SocketShutdown.Send); } catch (SocketException) { }
    }
}
";

/// Where the relay listens in the container.
const PROXY_URL: &str = "http://127.0.0.1:3128";

/// A script to run.
#[derive(Clone)]
pub struct Job {
    pub language: String,
    pub source: String,
    pub inputs: Json,
    pub timeout: Duration,
    /// Environment variables for the script. Podman takes their values
    /// from its own environment, so they're never on a command line.
    pub env: BTreeMap<String, String>,
    /// Values scrubbed from the output and logs.
    pub redact: Vec<String>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("language", &self.language)
            .field("timeout", &self.timeout)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// How it went.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub succeeded: bool,
    pub output: Option<Json>,
    pub error: Option<String>,
    pub exit_code: Option<i32>,
    pub logs: String,
}

impl Outcome {
    fn failed(error: impl Into<String>, exit_code: Option<i32>, logs: String) -> Self {
        Self {
            succeeded: false,
            output: None,
            error: Some(error.into()),
            exit_code,
            logs,
        }
    }
}

/// The rootless Podman sandbox.
#[derive(Debug, Clone)]
pub struct Podman {
    /// The `podman` binary.
    pub binary: String,
    pub pwsh_image: String,
    pub python_image: String,
    pub memory: String,
    pub cpus: String,
    pub pids: u32,
    /// The egress policy scripts' HTTP(S) goes through; `None`: no network
    /// at all.
    pub egress: Option<Arc<EgressConfig>>,
}

impl Default for Podman {
    fn default() -> Self {
        Self {
            binary: "podman".into(),
            pwsh_image: "mcr.microsoft.com/powershell:7.5-alpine-3.20".into(),
            python_image: "docker.io/library/python:3.13-alpine".into(),
            memory: "512m".into(),
            cpus: "1".into(),
            pids: 256,
            egress: Some(Arc::new(EgressConfig::default())),
        }
    }
}

impl Podman {
    /// Whether Podman works here.
    pub async fn available(&self) -> bool {
        Command::new(&self.binary)
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success())
    }

    /// The languages whose images are present locally.
    pub async fn languages(&self) -> Vec<String> {
        let mut found = Vec::new();
        for (language, image) in [("pwsh", &self.pwsh_image), ("python", &self.python_image)] {
            let present = Command::new(&self.binary)
                .args(["image", "exists", image])
                .status()
                .await
                .is_ok_and(|s| s.success());
            if present {
                found.push(language.to_owned());
            }
        }
        found
    }

    /// The arguments to `podman` for running `job` with its files in `dir`.
    fn args(&self, name: &str, job: &Job, dir: &Path) -> Option<Vec<String>> {
        let (image, command): (&str, &[&str]) = match job.language.as_str() {
            "pwsh" => (
                &self.pwsh_image,
                &[
                    "pwsh",
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-File",
                    "/huxley/in/wrapper.ps1",
                ],
            ),
            "python" => (
                &self.python_image,
                &["python3", "-I", "/huxley/in/wrapper.py"],
            ),
            _ => return None,
        };
        let mut args: Vec<String> = [
            "run",
            "--rm",
            "--name",
            name,
            "--network=none",
            "--read-only",
            "--cap-drop=all",
            "--security-opt=no-new-privileges",
            "--user=65534:65534",
            "--pull=never",
            "--tmpfs=/tmp:rw,nosuid,nodev,size=64m",
            "--env=HOME=/tmp",
            "--workdir=/tmp",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        args.push(format!("--memory={}", self.memory));
        args.push(format!("--cpus={}", self.cpus));
        args.push(format!("--pids-limit={}", self.pids));
        args.push(format!(
            "--volume={}:/huxley/in:ro",
            dir.join("in").display()
        ));
        args.push(format!(
            "--volume={}:/huxley/out:rw",
            dir.join("out").display()
        ));
        for name in job.env.keys() {
            args.push(format!("--env={name}"));
        }
        if self.egress.is_some() {
            args.push(format!(
                "--volume={}:/huxley/net:ro",
                dir.join("net").display()
            ));
            for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                args.push(format!("--env={name}={PROXY_URL}"));
            }
        }
        args.push(image.to_owned());
        args.extend(command.iter().map(|s| (*s).to_owned()));
        Some(args)
    }

    /// Runs `job` in a fresh container.
    pub async fn run(&self, job: &Job) -> Outcome {
        let outcome = match self.try_run(job).await {
            Ok(outcome) => outcome,
            Err(e) => Outcome::failed(format!("the sandbox failed: {e}"), None, String::new()),
        };
        scrub(outcome, &job.redact)
    }

    async fn try_run(&self, job: &Job) -> std::io::Result<Outcome> {
        let dir = tempdir()?;
        let result = self.run_in(job, &dir).await;
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    async fn run_in(&self, job: &Job, dir: &Path) -> std::io::Result<Outcome> {
        let name = format!("huxley-{}", huxley_core::new_id());
        let Some(args) = self.args(&name, job, dir) else {
            return Ok(Outcome::failed(
                format!("this runner can't run {}", job.language),
                None,
                String::new(),
            ));
        };
        prepare(job, dir)?;
        let output = dir.join("out");
        let proxy = CancellationToken::new();
        let _stop_proxy = proxy.clone().drop_guard();
        if let Some(policy) = &self.egress {
            let net = dir.join("net");
            std::fs::create_dir(&net)?;
            permissions(&net, 0o755)?;
            let socket = net.join("proxy.sock");
            let listener = tokio::net::UnixListener::bind(&socket)?;
            // The container's unprivileged user connects to it.
            permissions(&socket, 0o777)?;
            tokio::spawn(crate::proxy::serve(listener, policy.clone(), proxy.clone()));
        }

        let mut child = Command::new(&self.binary)
            .args(&args)
            .envs(&job.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let logs = tokio::spawn(async move {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let read_out = async {
                if let Some(mut s) = stdout {
                    let _ = s.read_to_end(&mut out).await;
                }
            };
            let read_err = async {
                if let Some(mut s) = stderr {
                    let _ = s.read_to_end(&mut err).await;
                }
            };
            tokio::join!(read_out, read_err);
            out.extend_from_slice(&err);
            out
        });
        let status = if let Ok(status) = tokio::time::timeout(job.timeout, child.wait()).await {
            status?
        } else {
            let _ = Command::new(&self.binary)
                .args(["kill", &name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            let _ = child.kill().await;
            let logs = clip(&logs.await.unwrap_or_default());
            return Ok(Outcome::failed(
                format!("the script didn't finish within {}s", job.timeout.as_secs()),
                None,
                logs,
            ));
        };
        let logs = clip(&logs.await.unwrap_or_default());
        let code = status.code();
        if !status.success() {
            return Ok(Outcome::failed(
                format!("the script exited with code {}", code.unwrap_or(-1)),
                code,
                logs,
            ));
        }
        let result = std::fs::read_to_string(output.join("result.json")).unwrap_or_default();
        let output = if result.trim().is_empty() {
            None
        } else {
            match serde_json::from_str(&result) {
                Ok(value) => Some(value),
                Err(_) => {
                    return Ok(Outcome::failed(
                        "the script's result isn't JSON",
                        code,
                        logs,
                    ));
                }
            }
        };
        Ok(Outcome {
            succeeded: true,
            output,
            error: None,
            exit_code: code,
            logs,
        })
    }
}

/// Writes the script, its wrapper and its inputs for the container.
fn prepare(job: &Job, dir: &Path) -> std::io::Result<()> {
    let (script, wrapper, wrapper_name) = if job.language == "pwsh" {
        ("script.ps1", PWSH_WRAPPER, "wrapper.ps1")
    } else {
        ("script.py", PYTHON_WRAPPER, "wrapper.py")
    };
    let input = dir.join("in");
    let output = dir.join("out");
    std::fs::create_dir(&input)?;
    std::fs::create_dir(&output)?;
    std::fs::write(input.join(script), &job.source)?;
    std::fs::write(input.join(wrapper_name), wrapper)?;
    std::fs::write(input.join("inputs.json"), job.inputs.to_string())?;
    std::fs::write(input.join("relay.cs"), PWSH_RELAY)?;
    permissions(&input, 0o755)?;
    for file in [script, wrapper_name, "inputs.json", "relay.cs"] {
        permissions(&input.join(file), 0o644)?;
    }
    // The container's unprivileged user writes its result here.
    permissions(&output, 0o777)
}

/// Masks the job's secrets in everything reported.
fn scrub(outcome: Outcome, redact: &[String]) -> Outcome {
    let secrets = || redact.iter().map(String::as_str);
    let text = |s: &str| match huxley_expr::redact(&Json::String(s.to_owned()), secrets()) {
        Json::String(s) => s,
        _ => String::new(),
    };
    Outcome {
        output: outcome.output.map(|o| huxley_expr::redact(&o, secrets())),
        error: outcome.error.as_deref().map(text),
        logs: text(&outcome.logs),
        ..outcome
    }
}

fn clip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_LOGS {
        return text.into_owned();
    }
    let mut end = MAX_LOGS;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (logs cut at {} KiB)", &text[..end], MAX_LOGS / 1024)
}

/// A private scratch directory for one run.
fn tempdir() -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("huxley-run-{}", huxley_core::new_id()));
    std::fs::create_dir(&dir)?;
    permissions(&dir, 0o711)?;
    Ok(dir)
}

#[cfg(unix)]
fn permissions(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn permissions(_: &Path, _: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_is_locked_down() {
        let podman = Podman::default();
        let job = Job {
            language: "python".into(),
            source: String::new(),
            inputs: Json::Null,
            timeout: Duration::from_secs(5),
            env: [("API_KEY".to_owned(), "sk-123456".to_owned())].into(),
            redact: Vec::new(),
        };
        let args = podman.args("x", &job, Path::new("/tmp/d")).unwrap();
        for flag in [
            "--network=none",
            "--read-only",
            "--cap-drop=all",
            "--security-opt=no-new-privileges",
            "--user=65534:65534",
            "--pull=never",
        ] {
            assert!(args.iter().any(|a| a == flag), "missing {flag}");
        }
        assert!(args.iter().any(|a| a == "--volume=/tmp/d/in:/huxley/in:ro"));
        // Values never go on the command line.
        assert!(args.iter().any(|a| a == "--env=API_KEY"));
        assert!(!args.iter().any(|a| a.contains("sk-123456")));
        assert!(
            podman
                .args(
                    "x",
                    &Job {
                        language: "bash".into(),
                        ..job
                    },
                    Path::new("/")
                )
                .is_none()
        );
    }

    #[test]
    fn long_logs_are_clipped() {
        let logs = clip(&vec![b'a'; MAX_LOGS + 10]);
        assert!(logs.len() < MAX_LOGS + 100);
        assert!(logs.ends_with("KiB)"));
    }
}
