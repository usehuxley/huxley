//! The Kubernetes Jobs sandbox, on a real cluster. Set
//! `HUXLEY_TEST_KUBE_CONTEXT` to a kubeconfig context (e.g. a kind
//! cluster) whose nodes have the python and pwsh script images and that
//! enforces `NetworkPolicy`; skipped otherwise.
//!
//! Each test gets its own namespace with the shipped network policies
//! (`deploy/kubernetes/network-policy.yaml`) and a stand-in egress proxy.

use std::time::Duration;

use huxley_runners::exec::Job;
use huxley_runners::kubernetes::Kubernetes;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::Api;
use kube::api::{DeleteParams, ListParams, PostParams};
use serde_json::{Value as Json, json};

/// Answers every proxied request with `via proxy <target>`.
const FAKE_PROXY: &str = r#"
import http.server
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = ("via proxy " + self.path).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
http.server.HTTPServer(("0.0.0.0", 3128), H).serve_forever()
"#;

struct Cluster {
    client: kube::Client,
    namespace: String,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let client = self.client.clone();
        let namespace = self.namespace.clone();
        // The client runs on the test's runtime: clean up on it.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let api: Api<Namespace> = Api::all(client);
                let _ = api.delete(&namespace, &DeleteParams::background()).await;
            });
        });
    }
}

async fn cluster(policies: bool) -> Option<Cluster> {
    let Ok(context) = std::env::var("HUXLEY_TEST_KUBE_CONTEXT") else {
        eprintln!("skipping: set HUXLEY_TEST_KUBE_CONTEXT");
        return None;
    };
    huxley_runners::install_crypto_provider();
    let options = kube::config::KubeConfigOptions {
        context: Some(context),
        ..Default::default()
    };
    let config = kube::Config::from_kubeconfig(&options).await.unwrap();
    let client = kube::Client::try_from(config).unwrap();
    let namespace = format!(
        "huxley-test-{}",
        &uuid::Uuid::now_v7().simple().to_string()[20..]
    );
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns: Namespace =
        serde_json::from_value(json!({ "metadata": { "name": namespace } })).unwrap();
    namespaces
        .create(&PostParams::default(), &ns)
        .await
        .unwrap();
    let cluster = Cluster { client, namespace };
    if policies {
        cluster.apply_policies().await;
        cluster.fake_proxy().await;
    }
    Some(cluster)
}

impl Cluster {
    async fn apply_policies(&self) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/kubernetes/network-policy.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), &self.namespace);
        for doc in text.split("\n---\n") {
            let value: Json = serde_saphyr::from_str(doc).unwrap();
            let policy: NetworkPolicy = serde_json::from_value(value).unwrap();
            api.create(&PostParams::default(), &policy).await.unwrap();
        }
    }

    async fn fake_proxy(&self) {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "proxy", "labels": { "app.kubernetes.io/name": "huxley-egress-proxy" } },
            "spec": { "containers": [{
                "name": "proxy",
                "image": "docker.io/library/python:3.13-alpine",
                "imagePullPolicy": "Never",
                "command": ["python3", "-c", FAKE_PROXY],
                "ports": [{ "containerPort": 3128 }]
            }] }
        }))
        .unwrap();
        pods.create(&PostParams::default(), &pod).await.unwrap();
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let service: Service = serde_json::from_value(json!({
            "metadata": { "name": "huxley-egress-proxy" },
            "spec": {
                "selector": { "app.kubernetes.io/name": "huxley-egress-proxy" },
                "ports": [{ "port": 3128, "targetPort": 3128 }]
            }
        }))
        .unwrap();
        services
            .create(&PostParams::default(), &service)
            .await
            .unwrap();
        // Wait until it's serving.
        for _ in 0..120 {
            let ready = pods
                .get("proxy")
                .await
                .ok()
                .and_then(|p| p.status)
                .is_some_and(|s| s.phase.as_deref() == Some("Running"));
            if ready {
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("the stand-in proxy didn't start");
    }

    async fn sandbox(&self) -> Kubernetes {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.namespace);
        let ip = services
            .get("huxley-egress-proxy")
            .await
            .ok()
            .and_then(|s| s.spec)
            .and_then(|s| s.cluster_ip);
        Kubernetes {
            client: self.client.clone(),
            namespace: self.namespace.clone(),
            pwsh_image: "mcr.microsoft.com/powershell:7.5-alpine-3.20".into(),
            python_image: "docker.io/library/python:3.13-alpine".into(),
            deno_image: "docker.io/denoland/deno:alpine-2.9.8".into(),
            memory: "256Mi".into(),
            cpus: "1".into(),
            runtime_class: None,
            proxy_url: ip.map(|ip| format!("http://{ip}:3128")),
        }
    }

    async fn leftovers(&self) -> usize {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let list = ListParams::default().labels("app.kubernetes.io/component=huxley-script");
        pods.list(&list).await.unwrap().items.len()
    }
}

fn job(language: &str, source: &str, inputs: Json) -> Job {
    Job {
        language: language.into(),
        source: source.into(),
        inputs,
        timeout: Duration::from_secs(60),
        env: std::collections::BTreeMap::new(),
        redact: Vec::new(),
    }
}

const PROBE: &str = r#"
import os, socket, urllib.request
# Straight away: the policy must already hold for a brand-new pod.
try:
    socket.create_connection(("10.96.0.1", 443), timeout=3)
    direct = "open"
except OSError:
    direct = "blocked"
print("checking with", os.environ["VAULT_KEY"])
with urllib.request.urlopen("http://example.invalid/hello", timeout=10) as r:
    proxied = r.read().decode()
result = {"direct": direct, "proxied": proxied, "name": inputs["name"],
          "uid": os.getuid(), "key": os.environ["VAULT_KEY"]}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn scripts_run_as_confined_jobs() {
    let Some(cluster) = cluster(true).await else {
        return;
    };
    let sandbox = cluster.sandbox().await;
    assert!(sandbox.check_policy("huxley-scripts").await.is_ok());
    let mut job = job("python", PROBE, json!({ "name": "ada" }));
    job.env = [("VAULT_KEY".to_owned(), "sk-live-ABCDEF".to_owned())].into();
    job.redact = vec!["sk-live-ABCDEF".into()];
    let (sink, mut lines) = tokio::sync::mpsc::unbounded_channel();
    let outcome = sandbox.run_with_logs(&job, Some(sink)).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(json!({
            "direct": "blocked",
            "proxied": "via proxy http://example.invalid/hello",
            "name": "ada",
            "uid": 65534,
            "key": "«redacted»"
        }))
    );
    assert!(
        outcome.logs.contains("checking with «redacted»"),
        "{}",
        outcome.logs
    );
    assert!(
        !outcome.logs.contains("huxley-result"),
        "the result line is taken out"
    );
    assert_eq!(
        lines.recv().await.as_deref(),
        Some("checking with «redacted»\n")
    );
    // The Job, its pod and its Secret are gone.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(cluster.leftovers().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_and_timeouts() {
    let Some(cluster) = cluster(true).await else {
        return;
    };
    let sandbox = cluster.sandbox().await;
    let failed = sandbox
        .run_with_logs(
            &job(
                "python",
                "import sys\nprint('boom', file=sys.stderr)\nsys.exit(3)",
                json!({}),
            ),
            None,
        )
        .await;
    assert!(!failed.succeeded);
    assert_eq!(failed.exit_code, Some(3));
    assert!(failed.logs.contains("boom"));

    let mut slow = job("python", "import time\ntime.sleep(120)", json!({}));
    slow.timeout = Duration::from_secs(5);
    let started = std::time::Instant::now();
    let timed_out = sandbox.run_with_logs(&slow, None).await;
    assert!(!timed_out.succeeded);
    assert!(
        timed_out.error.unwrap().contains("didn't finish"),
        "timed out"
    );
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[tokio::test(flavor = "multi_thread")]
async fn powershell_runs_too() {
    let Some(cluster) = cluster(true).await else {
        return;
    };
    let sandbox = cluster.sandbox().await;
    let job = job(
        "pwsh",
        "param([string]$Name)\n@{ greeting = \"hi $Name\" }",
        json!({ "Name": "ada" }),
    );
    let outcome = sandbox.run_with_logs(&job, None).await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(outcome.output, Some(json!({ "greeting": "hi ada" })));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_runner_wont_start_without_the_policy() {
    let Some(cluster) = cluster(false).await else {
        return;
    };
    let sandbox = cluster.sandbox().await;
    let refused = sandbox.check_policy("huxley-scripts").await.unwrap_err();
    assert!(refused.contains("isn't in namespace"), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn javascript_runs_too() {
    let Some(cluster) = cluster(true).await else {
        return;
    };
    let sandbox = cluster.sandbox().await;
    // Deno, confined: the network policy blocks direct connections, and
    // fetch goes through the egress proxy.
    let source = r#"
let direct = "blocked";
try {
  const c = await Promise.race([
    Deno.connect({ hostname: "10.96.0.1", port: 443 }),
    new Promise((_, reject) => setTimeout(() => reject(new Error("timeout")), 3000)),
  ]);
  c.close();
  direct = "open";
} catch {}
const r = await fetch("http://example.invalid/hello");
export default { direct, proxied: await r.text(), name: inputs.name };
"#;
    let outcome = sandbox
        .run_with_logs(&job("javascript", source, json!({ "name": "ada" })), None)
        .await;
    assert!(outcome.succeeded, "{outcome:?}");
    assert_eq!(
        outcome.output,
        Some(
            json!({ "direct": "blocked", "proxied": "via proxy http://example.invalid/hello", "name": "ada" })
        )
    );
}
