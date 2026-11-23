# ADR-026: On Kubernetes, scripts run as confined Jobs, with egress through a proxy enforced by NetworkPolicy

- **Status:** Accepted (the Kubernetes option of the sandbox decision, chosen by the maintainer, 2026-11-23)
- **Date:** 2026-11-23

## Context

Runners run scripts in rootless Podman by default. Installs on Kubernetes can have runners start Jobs instead (design doc §6.9), where nested containers are often unavailable or unwelcome. The Podman sandbox enforces egress by giving the container no network and handing it the egress proxy as a Unix socket. That doesn't work across pods.

## Decision

- **One Job per script**, in the runner's namespace (`huxley-scripts`), created and deleted by the runner through a namespaced Role. The runner gets no cluster-wide rights. The pod is confined like the Podman sandbox:
  - non-root (65534), a read-only root filesystem, all capabilities dropped, no privilege escalation, `seccomp: RuntimeDefault`
  - no service account token and no service links
  - CPU and memory limits, and `activeDeadlineSeconds` set to the step's timeout
  - an optional runtime class, such as gVisor

  It meets the `restricted` Pod Security Standard, which the namespace enforces.
- **Inputs and secrets** travel in a Secret that exists only for the Job. Files are mounted read-only, and environment variables come from `secretKeyRef`, so the values never appear in the pod spec.
- **Results and logs** come back over the pod's log stream. The wrapper prints the result after a per-task random marker, which the runner takes out. Other lines stream to Huxley as they arrive, scrubbed.
- **Egress is enforced with NetworkPolicy.**
  - Script pods may reach only the **egress proxy Deployment** (`huxley runner egress-proxy`, the same proxy and SSRF policy as the Podman sandbox, over TCP), and nothing may reach them.
  - Script pods get the proxy's **address, not a name**, so they need no DNS.
  - The runner **refuses to start** unless the policy that confines script pods exists.
  - Network plugins can apply a policy to a new pod a moment after it starts. On kind, a new pod's first connection did get out. So the wrapper **holds the script until a connection the policy forbids (the API server) is refused**, and fails the task if that doesn't happen within 60 seconds.
- **Manifests** in `deploy/kubernetes`: the namespace, both policies, the proxy, and the runner with its Role. The runner enrols itself on first start from a Secret.

## Consequences

- The cluster's network plugin must enforce NetworkPolicy. The runner checks that the policy object exists, and the wrapper checks that it is in force, but it can't check the plugin in general.
- HTTP(S) is still the only way out, as with Podman. On-prem protocols go through integrations.
- Each script costs a pod start, typically one to a few seconds, more than a Podman container.
- The tests run against a real cluster (kind) when `HUXLEY_TEST_KUBE_CONTEXT` is set: confinement, the policy race, results, logs, timeouts and PowerShell.
