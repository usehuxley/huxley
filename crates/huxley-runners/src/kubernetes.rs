//! Running scripts as Kubernetes Jobs: the sandbox option for installs on
//! Kubernetes (design doc §6.9, ADR-026). Each script is a Job whose pod
//! is locked down like the Podman sandbox (non-root, read-only root, no
//! capabilities, no service account token, resource limits, a deadline)
//! and optionally uses a runtime class such as gVisor.
//!
//! Its script, inputs and environment travel in a Secret that lives only
//! as long as the Job. A network policy (`deploy/kubernetes`) lets script
//! pods reach nothing but the egress proxy; the runner won't start without
//! it. The result comes back on stdout after a per-task marker, and the
//! logs stream from the pod as it runs.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{AsyncBufReadExt as _, StreamExt as _};
use k8s_openapi::ByteString;
use k8s_openapi::api::batch::v1::Job as KubeJob;
use k8s_openapi::api::core::v1::{Pod, Secret};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::api::{Api, DeleteParams, ListParams, LogParams, PostParams};
use serde_json::{Value as Json, json};

use crate::exec::{self, Job, LogSink, Outcome};

/// The label every script pod carries; the network policy selects it.
pub const SCRIPT_LABEL: &str = "app.kubernetes.io/component";
pub const SCRIPT_LABEL_VALUE: &str = "huxley-script";
/// How long a pod may take to start (scheduling, image pull) on top of
/// the script's own timeout.
const START_GRACE: Duration = Duration::from_secs(120);
/// The wrappers' exit code when the pod's network policy never applied.
const POLICY_NOT_IN_FORCE: i32 = 97;

/// The Kubernetes Jobs sandbox.
#[derive(Clone)]
pub struct Kubernetes {
    pub client: kube::Client,
    pub namespace: String,
    pub pwsh_image: String,
    pub python_image: String,
    pub memory: String,
    pub cpus: String,
    /// e.g. `gvisor`, when the cluster has it.
    pub runtime_class: Option<String>,
    /// `http://<address>:3128`: the egress proxy scripts' HTTP(S) uses;
    /// `None`: no egress (the network policy blocks everything).
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for Kubernetes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kubernetes")
            .field("namespace", &self.namespace)
            .field("runtime_class", &self.runtime_class)
            .field("proxy_url", &self.proxy_url)
            .finish_non_exhaustive()
    }
}

impl Kubernetes {
    /// Fails unless the network policy that confines script pods exists, so
    /// scripts never run with open egress by mistake.
    pub async fn check_policy(&self, name: &str) -> Result<(), String> {
        let api: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), &self.namespace);
        let policy = api
            .get_opt(name)
            .await
            .map_err(|e| format!("couldn't read NetworkPolicy {name}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "NetworkPolicy {name} isn't in namespace {}: apply deploy/kubernetes first",
                    self.namespace
                )
            })?;
        let spec = policy.spec.unwrap_or_default();
        let selects = spec
            .pod_selector
            .and_then(|s| s.match_labels)
            .is_some_and(|l| l.get(SCRIPT_LABEL).map(String::as_str) == Some(SCRIPT_LABEL_VALUE));
        let egress = spec
            .policy_types
            .is_some_and(|t| t.iter().any(|t| t == "Egress"));
        if selects && egress {
            Ok(())
        } else {
            Err(format!(
                "NetworkPolicy {name} must select {SCRIPT_LABEL}={SCRIPT_LABEL_VALUE} pods and restrict Egress"
            ))
        }
    }

    /// Runs `job` as a Kubernetes Job, streaming its output to `sink`.
    pub async fn run_with_logs(&self, job: &Job, sink: Option<LogSink>) -> Outcome {
        let name = format!(
            "huxley-script-{}",
            &huxley_core::new_id().simple().to_string()[16..]
        );
        let marker = format!("<<huxley-result-{}>>", huxley_core::new_id().simple());
        let outcome = match self.try_run(&name, &marker, job, sink).await {
            Ok(outcome) => outcome,
            Err(e) => Outcome::failed(format!("the sandbox failed: {e}"), None, String::new()),
        };
        self.clean_up(&name).await;
        exec::scrub(outcome, &job.redact)
    }

    async fn clean_up(&self, name: &str) {
        let jobs: Api<KubeJob> = Api::namespaced(self.client.clone(), &self.namespace);
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.namespace);
        let _ = jobs.delete(name, &DeleteParams::background()).await;
        let _ = secrets.delete(name, &DeleteParams::default()).await;
    }

    async fn try_run(
        &self,
        name: &str,
        marker: &str,
        job: &Job,
        sink: Option<LogSink>,
    ) -> Result<Outcome, kube::Error> {
        let Some(image) = self.image(&job.language) else {
            return Ok(Outcome::failed(
                format!("this runner can't run {}", job.language),
                None,
                String::new(),
            ));
        };
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.namespace);
        secrets
            .create(&PostParams::default(), &secret(name, job))
            .await?;
        let jobs: Api<KubeJob> = Api::namespaced(self.client.clone(), &self.namespace);
        let spec = self.job_spec(name, image, marker, job);
        jobs.create(
            &PostParams::default(),
            &serde_json::from_value(spec).map_err(kube::Error::SerdeError)?,
        )
        .await?;

        let deadline = tokio::time::Instant::now() + job.timeout + START_GRACE;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);
        let Some(pod) = self.started(&pods, name, deadline).await? else {
            return Ok(Outcome::failed(
                "the script's pod didn't start in time",
                None,
                String::new(),
            ));
        };
        if let Err(reason) = &pod {
            return Ok(Outcome::failed(reason.clone(), None, String::new()));
        }
        let pod_name = pod.unwrap_or_default();
        let logs = stream_logs(&pods, &pod_name, marker, &job.redact, sink);
        let (logs, result) = match tokio::time::timeout_at(deadline, logs).await {
            Ok(read) => read?,
            Err(_) => (String::new(), None),
        };
        let mut state = terminated(&pods, &pod_name, deadline).await?;
        // Killed at the Job's deadline: a timeout, whatever the exit code.
        let deadline_exceeded = jobs
            .get_opt(name)
            .await?
            .and_then(|j| j.status)
            .and_then(|s| s.conditions)
            .is_some_and(|c| {
                c.iter()
                    .any(|c| c.reason.as_deref() == Some("DeadlineExceeded"))
            });
        if deadline_exceeded {
            state = None;
        }
        if state == Some(POLICY_NOT_IN_FORCE) {
            return Ok(Outcome::failed(
                "the script pod's network policy wasn't in force, so the script didn't run",
                None,
                logs,
            ));
        }
        Ok(outcome(state, logs, result.as_deref(), job.timeout))
    }

    fn image(&self, language: &str) -> Option<&str> {
        match language {
            "pwsh" => Some(&self.pwsh_image),
            "python" => Some(&self.python_image),
            _ => None,
        }
    }

    /// The Job: one locked-down pod, no retries, a deadline.
    fn job_spec(&self, name: &str, image: &str, marker: &str, job: &Job) -> Json {
        let command: Vec<&str> = if job.language == "pwsh" {
            vec![
                "pwsh",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                "/huxley/in/wrapper.ps1",
            ]
        } else {
            vec!["python3", "-I", "-u", "/huxley/in/wrapper.py"]
        };
        let mut env = vec![
            json!({ "name": "HOME", "value": "/tmp" }),
            json!({ "name": "HUXLEY_RESULT_MARKER", "value": marker }),
            // The API server: the policy forbids it, so reaching it means
            // the policy isn't in force for this pod yet.
            json!({
                "name": "HUXLEY_EGRESS_PROBE",
                "value": "$(KUBERNETES_SERVICE_HOST):$(KUBERNETES_SERVICE_PORT)"
            }),
        ];
        for key in job.env.keys() {
            env.push(json!({
                "name": key,
                "valueFrom": { "secretKeyRef": { "name": name, "key": format!("env-{key}") } }
            }));
        }
        if let Some(proxy) = &self.proxy_url {
            for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
                env.push(json!({ "name": key, "value": proxy }));
            }
        }
        let files = [
            "script.ps1",
            "script.py",
            "wrapper.ps1",
            "wrapper.py",
            "inputs.json",
        ];
        let mut pod = json!({
            "restartPolicy": "Never",
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "securityContext": {
                "runAsNonRoot": true,
                "runAsUser": 65534,
                "runAsGroup": 65534,
                "seccompProfile": { "type": "RuntimeDefault" }
            },
            "containers": [{
                "name": "script",
                "image": image,
                "imagePullPolicy": "IfNotPresent",
                "command": command,
                "workingDir": "/tmp",
                "env": env,
                "securityContext": {
                    "readOnlyRootFilesystem": true,
                    "allowPrivilegeEscalation": false,
                    "capabilities": { "drop": ["ALL"] }
                },
                "resources": {
                    "limits": { "memory": self.memory, "cpu": self.cpus },
                    "requests": { "memory": "64Mi", "cpu": "50m" }
                },
                "volumeMounts": [
                    { "name": "in", "mountPath": "/huxley/in", "readOnly": true },
                    { "name": "out", "mountPath": "/huxley/out" },
                    { "name": "tmp", "mountPath": "/tmp" }
                ]
            }],
            "volumes": [
                { "name": "in", "secret": {
                    "secretName": name,
                    "defaultMode": 0o444,
                    "items": files.iter().map(|f| json!({ "key": f, "path": f })).collect::<Vec<_>>()
                } },
                { "name": "out", "emptyDir": { "sizeLimit": "16Mi" } },
                { "name": "tmp", "emptyDir": { "sizeLimit": "64Mi" } }
            ]
        });
        if let Some(class) = &self.runtime_class {
            pod["runtimeClassName"] = json!(class);
        }
        json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": { "name": name, "labels": { SCRIPT_LABEL: SCRIPT_LABEL_VALUE } },
            "spec": {
                "backoffLimit": 0,
                "activeDeadlineSeconds": job.timeout.as_secs().max(1),
                "ttlSecondsAfterFinished": 300,
                "template": {
                    "metadata": { "labels": { SCRIPT_LABEL: SCRIPT_LABEL_VALUE } },
                    "spec": pod
                }
            }
        })
    }

    /// Waits for the Job's pod to start (or end). `Ok(None)`: not in time;
    /// `Some(Err(reason))`: it can't start (e.g. the image can't be pulled).
    async fn started(
        &self,
        pods: &Api<Pod>,
        job: &str,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Result<String, String>>, kube::Error> {
        let list = ListParams::default().labels(&format!("job-name={job}"));
        while tokio::time::Instant::now() < deadline {
            for pod in pods.list(&list).await?.items {
                let name = pod.metadata.name.clone().unwrap_or_default();
                let status = pod.status.unwrap_or_default();
                if matches!(
                    status.phase.as_deref(),
                    Some("Running" | "Succeeded" | "Failed")
                ) {
                    return Ok(Some(Ok(name)));
                }
                for container in status.container_statuses.unwrap_or_default() {
                    let waiting = container.state.and_then(|s| s.waiting);
                    if let Some(reason) = waiting.and_then(|w| w.reason)
                        && matches!(
                            reason.as_str(),
                            "ErrImagePull"
                                | "ImagePullBackOff"
                                | "InvalidImageName"
                                | "CreateContainerConfigError"
                        )
                    {
                        return Ok(Some(Err(format!("the script's pod can't start: {reason}"))));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(None)
    }
}

fn secret(name: &str, job: &Job) -> Secret {
    let (script, wrapper_name, wrapper) = if job.language == "pwsh" {
        ("script.ps1", "wrapper.ps1", exec::PWSH_WRAPPER)
    } else {
        ("script.py", "wrapper.py", exec::PYTHON_WRAPPER)
    };
    let mut data: BTreeMap<String, ByteString> = BTreeMap::new();
    let mut put = |k: &str, v: &str| {
        data.insert(k.to_owned(), ByteString(v.as_bytes().to_vec()));
    };
    // Every file key the volume lists exists, so the mount never fails.
    for file in ["script.ps1", "script.py", "wrapper.ps1", "wrapper.py"] {
        put(file, "");
    }
    put(script, &job.source);
    put(wrapper_name, wrapper);
    put("inputs.json", &job.inputs.to_string());
    for (key, value) in &job.env {
        put(&format!("env-{key}"), value);
    }
    let mut labels = BTreeMap::new();
    labels.insert(SCRIPT_LABEL.to_owned(), SCRIPT_LABEL_VALUE.to_owned());
    Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(name.to_owned()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("Opaque".into()),
        ..Default::default()
    }
}

/// Follows the pod's output: lines go (scrubbed) to `sink`, the result
/// line after `marker` is taken out. Returns the logs and the result text.
async fn stream_logs(
    pods: &Api<Pod>,
    pod: &str,
    marker: &str,
    redact: &[String],
    sink: Option<LogSink>,
) -> Result<(String, Option<String>), kube::Error> {
    let params = LogParams {
        follow: true,
        ..LogParams::default()
    };
    let mut lines = pods.log_stream(pod, &params).await?.lines();
    let mut logs = String::new();
    let mut result = None;
    while let Some(line) = lines.next().await {
        let Ok(line) = line else { break };
        if let Some(rest) = line.strip_prefix(marker) {
            result = Some(rest.to_owned());
            continue;
        }
        if logs.len() < exec::MAX_LOGS {
            if let Some(sink) = &sink {
                let _ = sink.send(exec::scrub_text(&format!("{line}\n"), redact));
            }
            logs.push_str(&line);
            logs.push('\n');
        }
    }
    Ok((logs, result))
}

/// How the pod's container ended: `Some(exit code)`, or `None` if it was
/// stopped (deadline) or never finished in time.
async fn terminated(
    pods: &Api<Pod>,
    pod: &str,
    deadline: tokio::time::Instant,
) -> Result<Option<i32>, kube::Error> {
    loop {
        let found = pods.get_opt(pod).await?;
        let state = found
            .and_then(|p| p.status)
            .and_then(|s| s.container_statuses)
            .and_then(|c| c.into_iter().next())
            .and_then(|c| c.state)
            .and_then(|s| s.terminated);
        if let Some(state) = state {
            let killed = matches!(state.reason.as_deref(), Some("DeadlineExceeded"));
            return Ok(if killed { None } else { Some(state.exit_code) });
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn outcome(exit: Option<i32>, logs: String, result: Option<&str>, timeout: Duration) -> Outcome {
    let Some(code) = exit else {
        return Outcome::failed(
            format!("the script didn't finish within {}s", timeout.as_secs()),
            None,
            logs,
        );
    };
    if code != 0 {
        return Outcome::failed(
            format!("the script exited with code {code}"),
            Some(code),
            logs,
        );
    }
    let output = match result.map(str::trim) {
        None | Some("") => None,
        Some(text) => match serde_json::from_str(text) {
            Ok(value) => Some(value),
            Err(_) => return Outcome::failed("the script's result isn't JSON", Some(code), logs),
        },
    };
    Outcome {
        succeeded: true,
        output,
        error: None,
        exit_code: Some(code),
        logs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_from_exit_codes_and_results() {
        let t = Duration::from_secs(5);
        let ok = outcome(Some(0), "hi\n".into(), Some("{\"a\":1}"), t);
        assert!(ok.succeeded);
        assert_eq!(ok.output, Some(json!({ "a": 1 })));
        assert_eq!(
            outcome(Some(0), String::new(), Some("null"), t).output,
            Some(Json::Null)
        );
        assert_eq!(outcome(Some(3), String::new(), None, t).exit_code, Some(3));
        assert!(
            outcome(None, String::new(), None, t)
                .error
                .unwrap()
                .contains("didn't finish")
        );
        assert!(!outcome(Some(0), String::new(), Some("nope"), t).succeeded);
    }
}
