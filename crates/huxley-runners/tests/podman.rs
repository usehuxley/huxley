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
