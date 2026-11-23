//! Where scripts run: rootless Podman (the default) or Kubernetes Jobs.

use crate::exec::{Job, LogSink, Outcome, Podman};
use crate::kubernetes::Kubernetes;

#[derive(Debug, Clone)]
pub enum Sandbox {
    Podman(Podman),
    Kubernetes(Kubernetes),
}

impl Sandbox {
    /// Runs `job`, streaming its (scrubbed) output to `sink`.
    pub async fn run_with_logs(&self, job: &Job, sink: Option<LogSink>) -> Outcome {
        match self {
            Self::Podman(podman) => podman.run_with_logs(job, sink).await,
            Self::Kubernetes(kubernetes) => kubernetes.run_with_logs(job, sink).await,
        }
    }
}
