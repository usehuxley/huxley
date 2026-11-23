# Script runners

Runners execute library scripts in sandboxes, separate from the API and workers (design doc §6.9). They connect to Huxley's **runner gateway**, a gRPC listener on its own port, with a client certificate (ADR-025).

A runner asks the gateway for work, runs each script in a fresh **rootless Podman** container, and reports the result. The `script` workflow step queues that work (see [workflows](workflows.md#steps)). Kubernetes Jobs as a sandbox and live log streaming come next.

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
```

It only offers the languages whose images are present. Choose other images with `--pwsh-image` and `--python-image`, for example ones with your modules preinstalled. `--concurrency` (default 2) sets how many scripts run at once. Logs, meaning stdout and stderr up to 64 KB, are kept with the task, and a failed step's error includes the last 4 KB of them.

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
