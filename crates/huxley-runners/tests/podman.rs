//! The Podman sandbox, for real. Skipped where Podman or the images are
//! missing, unless `HUXLEY_REQUIRE_PODMAN_TESTS` is set.

use std::time::Duration;

use huxley_runners::exec::{Job, Podman};
use serde_json::json;

async fn podman(language: &str) -> Option<Podman> {
    let podman = Podman::default();
    let ready = podman.available().await && podman.languages().await.iter().any(|l| l == language);
    if !ready {
        assert!(
            std::env::var_os("HUXLEY_REQUIRE_PODMAN_TESTS").is_none(),
            "Podman or the {language} image isn't available"
        );
        eprintln!("skipping: Podman or the {language} image isn't available");
        return None;
    }
    Some(podman)
}

fn python(source: &str, inputs: serde_json::Value) -> Job {
    Job {
        language: "python".into(),
        source: source.into(),
        inputs,
        timeout: Duration::from_secs(30),
        env: std::collections::BTreeMap::new(),
        redact: Vec::new(),
    }
}

#[tokio::test]
async fn python_scripts_get_inputs_and_return_results() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let outcome = podman
        .run(&python(
            "print('hello', inputs['name'])\nresult = {'greeting': 'hi ' + inputs['name']}",
            json!({ "name": "ada" }),
        ))
        .await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(outcome.output, Some(json!({ "greeting": "hi ada" })));
    assert!(outcome.logs.contains("hello ada"));
}

#[tokio::test]
async fn the_sandbox_has_no_network_and_a_read_only_root() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let source = r#"
import os, socket
try:
    socket.create_connection(("1.1.1.1", 53), timeout=3)
    net = "open"
except OSError:
    net = "blocked"
try:
    open("/etc/huxley-test", "w")
    root = "writable"
except OSError:
    root = "read-only"
result = {"net": net, "root": root, "uid": os.getuid()}
"#;
    let outcome = podman.run(&python(source, json!({}))).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(json!({ "net": "blocked", "root": "read-only", "uid": 65534 }))
    );
}

#[tokio::test]
async fn failures_and_timeouts_are_reported() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let failed = podman
        .run(&python(
            "import sys\nprint('boom', file=sys.stderr)\nsys.exit(3)",
            json!({}),
        ))
        .await;
    assert!(!failed.succeeded);
    assert_eq!(failed.exit_code, Some(3));
    assert!(failed.logs.contains("boom"));

    let mut slow = python("import time\ntime.sleep(60)", json!({}));
    slow.timeout = Duration::from_secs(3);
    let started = std::time::Instant::now();
    let timed_out = podman.run(&slow).await;
    assert!(!timed_out.succeeded);
    assert!(timed_out.error.unwrap().contains("didn't finish"));
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn powershell_scripts_get_parameters() {
    let Some(podman) = podman("pwsh").await else {
        return;
    };
    let job = Job {
        language: "pwsh".into(),
        source:
            "param([string]$Name, [int]$Count)\n@{ greeting = \"$env:GREETING $Name\"; twice = $Count * 2 }"
                .into(),
        inputs: json!({ "Name": "ada", "Count": 21 }),
        timeout: Duration::from_secs(60),
        env: [("GREETING".to_owned(), "hi".to_owned())].into(),
        redact: Vec::new(),
    };
    let outcome = podman.run(&job).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(json!({ "greeting": "hi ada", "twice": 42 }))
    );
}

#[tokio::test]
async fn secrets_arrive_as_environment_and_are_scrubbed() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let mut job = python(
        "import os\nprint('key is', os.environ['VAULT_KEY'])\nresult = {'echo': os.environ['VAULT_KEY'], 'region': os.environ['REGION']}",
        json!({}),
    );
    job.env = [
        ("VAULT_KEY".to_owned(), "sk-live-ABCDEF".to_owned()),
        ("REGION".to_owned(), "eu".to_owned()),
    ]
    .into();
    job.redact = vec!["sk-live-ABCDEF".into()];
    let outcome = podman.run(&job).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(json!({ "echo": "«redacted»", "region": "eu" }))
    );
    assert!(
        outcome.logs.contains("key is «redacted»"),
        "{}",
        outcome.logs
    );
    assert!(!outcome.logs.contains("ABCDEF"));
}

/// A host-side HTTP server answering `hello from the host`.
async fn host_server() -> u16 {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                let body = "hello from the host";
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

fn allowing_localhost(mut podman: Podman) -> Podman {
    podman.egress = Some(std::sync::Arc::new(huxley_engine::EgressConfig {
        private_hosts: vec!["127.0.0.1".into()],
        ..huxley_engine::EgressConfig::default()
    }));
    podman
}

const FETCH: &str = r#"
import urllib.request, urllib.error
try:
    with urllib.request.urlopen(inputs["url"], timeout=10) as r:
        result = {"status": r.status, "body": r.read().decode()}
except urllib.error.HTTPError as e:
    result = {"status": e.code, "body": e.read().decode()}
"#;

#[tokio::test]
async fn http_goes_through_the_egress_policy() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let port = host_server().await;
    let url = format!("http://127.0.0.1:{port}/");
    // The default policy: private addresses are refused.
    let refused = podman.run(&python(FETCH, json!({ "url": url }))).await;
    assert!(refused.succeeded, "{refused:?}");
    let output = refused.output.unwrap();
    assert_eq!(output["status"], 403, "{output}");
    assert!(
        output["body"]
            .as_str()
            .unwrap()
            .contains("isn't a public address")
    );
    // Allowed by the policy: it gets through.
    let allowed = allowing_localhost(podman)
        .run(&python(FETCH, json!({ "url": url })))
        .await;
    assert_eq!(
        allowed.output,
        Some(json!({ "status": 200, "body": "hello from the host" })),
        "{allowed:?}"
    );
}

#[tokio::test]
async fn without_egress_there_is_no_way_out() {
    let Some(mut podman) = podman("python").await else {
        return;
    };
    podman.egress = None;
    let port = host_server().await;
    let source = r#"
import urllib.request
try:
    urllib.request.urlopen(inputs["url"], timeout=5)
    result = "reached"
except Exception as e:
    result = "unreachable"
"#;
    let outcome = podman
        .run(&python(
            source,
            json!({ "url": format!("http://127.0.0.1:{port}/") }),
        ))
        .await;
    assert_eq!(outcome.output, Some(json!("unreachable")), "{outcome:?}");
}

#[tokio::test]
async fn powershell_http_goes_through_the_egress_policy() {
    let Some(podman) = podman("pwsh").await else {
        return;
    };
    let port = host_server().await;
    let job = Job {
        language: "pwsh".into(),
        source: "param([string]$Url)\n(Invoke-WebRequest -Uri $Url -UseBasicParsing).Content"
            .into(),
        inputs: json!({ "Url": format!("http://127.0.0.1:{port}/") }),
        timeout: Duration::from_secs(90),
        env: std::collections::BTreeMap::new(),
        redact: Vec::new(),
    };
    let outcome = allowing_localhost(podman).run(&job).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(outcome.output, Some(json!("hello from the host")));
}

/// Needs the internet: `cargo test -p huxley-runners --test podman -- --ignored`.
#[tokio::test]
#[ignore = "needs internet access"]
async fn https_reaches_the_internet_through_the_proxy() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let outcome = podman
        .run(&python(FETCH, json!({ "url": "https://example.com/" })))
        .await;
    let output = outcome.output.clone().unwrap_or_default();
    assert_eq!(output["status"], 200, "{outcome:?}");
    assert!(output["body"].as_str().unwrap().contains("Example Domain"));
}

#[tokio::test]
async fn output_streams_while_the_script_runs() {
    let Some(podman) = podman("python").await else {
        return;
    };
    let mut job = python(
        "import time\nprint('starting with sk-live-ABCDEF')\ntime.sleep(4)\nprint('done')\nresult = 1",
        json!({}),
    );
    job.redact = vec!["sk-live-ABCDEF".into()];
    let (sink, mut lines) = tokio::sync::mpsc::unbounded_channel();
    let running = tokio::spawn(async move { podman.run_with_logs(&job, Some(sink)).await });
    // The first line arrives well before the script finishes, scrubbed.
    let first = tokio::time::timeout(Duration::from_secs(3), lines.recv())
        .await
        .expect("no output while the script ran")
        .unwrap();
    assert_eq!(first, "starting with «redacted»\n");
    assert!(!running.is_finished());
    let outcome = running.await.unwrap();
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(lines.recv().await.as_deref(), Some("done\n"));
    assert!(outcome.logs.contains("starting with «redacted»\ndone"));
}

fn javascript(source: &str, inputs: serde_json::Value) -> Job {
    Job {
        language: "javascript".into(),
        ..python(source, inputs)
    }
}

#[tokio::test]
async fn javascript_scripts_run_with_node() {
    let Some(podman) = podman("javascript").await else {
        return;
    };
    // export default: a function of the inputs (async is fine).
    let outcome = podman
        .run(&javascript(
            "console.log('hello', inputs.name);\nexport default async ({ name }) => ({ greeting: `hi ${name}`, typed: typeof name });",
            json!({ "name": "ada" }),
        ))
        .await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(json!({ "greeting": "hi ada", "typed": "string" }))
    );
    assert!(outcome.logs.contains("hello ada"));
    assert!(
        !outcome.logs.contains("ExperimentalWarning"),
        "{}",
        outcome.logs
    );
    // Or globalThis.result.
    let outcome = podman
        .run(&javascript("globalThis.result = [1, 2, 3];", json!({})))
        .await;
    assert_eq!(outcome.output, Some(json!([1, 2, 3])), "{outcome:?}");
    // A throw fails the script, with its message in the logs.
    let failed = podman
        .run(&javascript("throw new Error('nope');", json!({})))
        .await;
    assert!(!failed.succeeded);
    assert!(failed.logs.contains("Error: nope"), "{failed:?}");
}

#[tokio::test]
async fn javascript_is_confined_like_the_rest() {
    let Some(podman) = podman("javascript").await else {
        return;
    };
    let source = r#"
import fs from "node:fs";
import net from "node:net";
const direct = await new Promise((resolve) => {
  const s = net.connect({ host: "1.1.1.1", port: 53 });
  s.setTimeout(3000, () => { s.destroy(); resolve("blocked"); });
  s.once("connect", () => { s.destroy(); resolve("open"); });
  s.once("error", () => resolve("blocked"));
});
let host = "denied";
try { fs.readFileSync("/etc/passwd"); host = "readable"; } catch {}
let tmp = "denied";
try { fs.writeFileSync("/tmp/x", "x"); tmp = "writable"; } catch {}
let child = "denied";
try { (await import("node:child_process")).execSync("id"); child = "allowed"; } catch {}
export default { direct, host, tmp, child, uid: process.getuid() };
"#;
    let without = Podman {
        egress: None,
        ..podman.clone()
    };
    let outcome = without.run(&javascript(source, json!({}))).await;
    assert!(outcome.succeeded, "{outcome:?}");
    // Node's own permissions also keep the script to the job's files, and
    // no subprocesses.
    assert_eq!(
        outcome.output,
        Some(
            json!({ "direct": "blocked", "host": "denied", "tmp": "denied", "child": "denied", "uid": 65534 })
        )
    );

    // fetch() goes through the egress policy, like other languages.
    let port = host_server().await;
    let fetch = r"
const r = await fetch(inputs.url);
export default { status: r.status, body: await r.text() };
";
    let url = format!("http://127.0.0.1:{port}/");
    let refused = podman.run(&javascript(fetch, json!({ "url": url }))).await;
    assert_eq!(
        refused.output.as_ref().map(|o| o["status"].clone()),
        Some(json!(403)),
        "{refused:?}"
    );
    let allowed = allowing_localhost(podman)
        .run(&javascript(fetch, json!({ "url": url })))
        .await;
    assert_eq!(
        allowed.output,
        Some(json!({ "status": 200, "body": "hello from the host" })),
        "{allowed:?}"
    );
}
