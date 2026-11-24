# Script runners

Runners execute library scripts in sandboxes, separate from the API and workers (design doc §6.9). They connect to Huxley's **runner gateway**, a gRPC listener on its own port, with a client certificate (ADR-025).

A runner asks the gateway for work, runs each script in a fresh **rootless Podman** container, and reports the result. The `script` workflow step queues that work (see [workflows](workflows.md#steps)). On Kubernetes, scripts can run as Jobs instead (see below).

## Set up the gateway

Set `runners.bind` (e.g. `HUXLEY_RUNNERS__BIND=0.0.0.0:8443`) on the instance serving the API. Also set:

- `runners.hosts`: the names runners use to reach it
- `runners.public_url`: the address runners connect to, e.g. `https://runners.example.com:8443`

The gateway's server certificate comes from Huxley's runner CA, which runners receive when they enrol. Put the gateway behind a TCP (not HTTP) load balancer if you need one, because TLS ends at the gateway.

## Enrol a runner (Huxley's CA)

1. In **Settings → Script runners**, create a token for the runner's name. It's shown once, works once, and lasts 24 hours.
2. On the runner's host:

   ```sh
   huxley runner enroll --server https://huxley.example.com --token hxr_…
   huxley runner run
   ```

`enroll` makes the runner's own key and sends only a certificate request. It stores the key, the certificate (valid 7 days), the CA and the gateway address in `--state` (default `/var/lib/huxley-runner`), readable only by the runner's user. `run` sends a heartbeat every 30 seconds and renews the certificate when less than two days are left. Each renewal replaces the previous certificate.

## Run it in a container

`ghcr.io/usehuxley/runner` is `huxley runner` with rootless Podman built in. Run it with what nested rootless Podman needs, and **never** with a Docker or Podman socket:

```sh
docker run -d --name huxley-runner \
  --device /dev/fuse --security-opt label=disable \
  -v runner-state:/var/lib/huxley-runner \
  -v runner-images:/home/podman/.local/share/containers \
  ghcr.io/usehuxley/runner run --pull-images
```

Enrol it once beforehand, with the same volumes, by running `ghcr.io/usehuxley/runner enroll --server https://huxley.example.com --token hxr_…`. `--pull-images` fetches the PowerShell, Python and Deno (JavaScript) images when it starts. Scripts never cause a pull.

The reference Compose deployment has it as the `runner` profile. Enrol it with `docker compose run --rm runner enroll --server http://huxley:8080 --token hxr_…`, then start it with `docker compose --profile runner up -d`.

## The sandbox

Each script runs in its own container, removed afterwards, with:

- **no network of its own** (`--network=none`): HTTP(S) goes through the egress proxy (below), and nothing else gets out
- a **read-only root filesystem**, plus a 64 MB `/tmp`
- **no capabilities**, `no-new-privileges`, and the unprivileged user `65534`
- limits: **512 MB memory and 1 CPU** by default (`--memory`, `--cpus`), and 256 processes
- the step's **timeout**, after which the container is killed

The runner needs Podman (rootless) and the images, which it never pulls by itself:

```sh
podman pull mcr.microsoft.com/powershell:7.5-alpine-3.20
podman pull docker.io/library/python:3.13-alpine
podman pull docker.io/denoland/deno:alpine-2.9.8
```

It only offers the languages whose images are present. Choose other images with `--pwsh-image`, `--python-image` and `--deno-image`, for example ones with your modules preinstalled. `--concurrency` (default 2) sets how many scripts run at once. Logs (stdout and stderr up to 64 KB) are streamed while the script runs: the runner sends new lines about every second, and the run page shows them as they arrive. The whole log replaces them when the script finishes, and a failed step's error includes its last 4 KB. Each streamed line is scrubbed by the runner, and again by Huxley.

**JavaScript:**

- **Runtime:** scripts run with Deno, which also accepts TypeScript syntax.
- **Permissions inside the container:** on top of the container's confinement, Deno lets a script read only the job's files, write only its result, read environment variables, and make network calls (which go through the egress proxy).
- **No system information** (`Deno.uid()` and the like) and no subprocesses.

## Network egress

Scripts reach the internet only through the runner's **egress proxy**. It applies the same policy as HTTP steps:

- **Public addresses only**, by default.
- **The proxy resolves names itself** and connects only to checked addresses, so DNS rebinding gets nowhere.
- **Link-local addresses** (cloud metadata such as `169.254.169.254`) are never reachable.

The container has no network interface besides loopback. The proxy is a Unix socket mounted into it, and a relay in the script's wrapper listens on `127.0.0.1:3128`. `HTTP_PROXY` and `HTTPS_PROXY` point at the relay, so `Invoke-RestMethod`, the Microsoft Graph modules, `urllib` and `requests` use it without changes.

Only HTTP and HTTPS work: there are no raw sockets, LDAP or SMB. Work inside a customer's network goes through an integration with a presence there (design doc §6.9).

Runner flags:

| Flag | Effect |
|---|---|
| `--egress-allow-private` | Allow private addresses, such as an internal API. Link-local stays blocked. |
| `--egress-private-host` | Host names that may resolve to private addresses, e.g. `*.corp.example`. Repeatable. |
| `--egress-denied-host` | Host names scripts may never reach |
| `--no-network` | No egress at all |

## On Kubernetes: Jobs

With `--sandbox kubernetes`, each script runs as a Job in the runner's namespace instead of a Podman container (ADR-026).

- **The pod is confined like the Podman sandbox:** non-root, a read-only root, no capabilities, no service account token, CPU and memory limits, and the step's timeout as its deadline. `--k8s-runtime-class gvisor` adds gVisor where the cluster has it.
- **Inputs and secrets** travel in a Secret that lives only as long as the Job.
- **Egress:** a NetworkPolicy lets script pods reach only the **egress proxy** Deployment (`huxley runner egress-proxy`, with the same policy and flags as above), and nothing else. The runner won't start without that policy. Each script waits until the policy is in force for its pod before any of its code runs. **Your cluster's network plugin must enforce NetworkPolicy.**

`deploy/kubernetes` sets it all up in the `huxley-scripts` namespace. The runner enrols itself on first start:

```sh
kubectl create namespace huxley-scripts
kubectl create secret generic huxley-runner-enrolment -n huxley-scripts \
  --from-literal=server=https://huxley.example.com --from-literal=token=hxr_…
kubectl apply -k deploy/kubernetes
```

The script images come from the cluster's registry access: `--pwsh-image`, `--python-image` and `--deno-image` work as with Podman. A runner outside the cluster can also use it, with a kubeconfig and `--k8s-namespace`.

## Credentials

A step's `env` and `connections` arrive as environment variables. Connection tokens are fetched when the runner claims the task, so they're fresh.

- Podman takes their values from its own environment, never from a command line.
- The runner scrubs every secret value and credential from the output, error and logs it reports.
- Huxley scrubs them again when it records the report, whatever the runner sent.
- They're stored encrypted until the task is claimed.

## Use your own CA

Set `runners.client_ca_file` to your CA's PEM bundle. Then, under **Settings → Script runners → Use your own CA**, register each runner under its certificate's common name. Your CA issues and renews those certificates. Huxley only checks that the name is registered and not revoked.

## Revoke

**Revoke** on a runner refuses it from its next call to the gateway. The gateway looks up the runner on every call, so a revoked runner is refused even though its certificate hasn't expired.

## Permissions

`platform:runners:manage` (an instance permission) allows enrolling, registering and revoking runners. Owners and admins have it.
