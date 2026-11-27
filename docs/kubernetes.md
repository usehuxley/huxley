# Kubernetes

The Helm chart in `deploy/helm/huxley` runs Huxley on Kubernetes. For a single server, use the Compose deployment in `deploy/compose` instead.

The chart deploys:

| Part | What it runs |
|---|---|
| **API** | `huxley serve --roles api`: the REST API, SCIM, MCP, webhooks, and the script runner gateway. It scales on CPU when `api.autoscaling.enabled` is set. |
| **Worker pools** | `--roles worker`, one Deployment per pool. Each pool takes only its job kinds; see below. |
| **Scheduler** | `--roles scheduler`: timers, schedules, retention, audit sealing and streaming. |
| **Web, Portal** | The staff app and the client portal. |
| **Migrations** | A Job that runs before every install and upgrade. |
| **PgBouncer** | Optional: transaction pooling between Huxley and PostgreSQL. |

You bring PostgreSQL 16 or later (managed or your own), object storage, and an ingress controller. Script runners deploy separately; see [Script runners](./runners.md).

## Installing

No images are published yet. Build them from `docker/` (`huxley`, `web`, `portal`), push them to your registry, and set `image.registry`.

```sh
kubectl create namespace huxley
kubectl -n huxley create secret generic huxley-db --from-literal=password='…'
kubectl -n huxley create secret generic huxley-kek --from-literal=kek="$(openssl rand -base64 32)"

helm install huxley deploy/helm/huxley -n huxley \
  --set image.registry=registry.example.com/huxley \
  --set host=huxley.example.com --set portalHost=portal.example.com \
  --set database.host=postgres.example.internal --set database.existingSecret=huxley-db \
  --set kek.existingSecret=huxley-kek \
  --set storage.url=s3://huxley-objects --set storage.optionsSecret=huxley-storage
```

Then set up the instance once. The command the chart prints after installing does it, and it prints a generated password for the first user.

**Back up the key-encryption key with the database.** Without it, encrypted secrets (connections, MFA, vault entries) can't be recovered.

### Storage

Attachments and large workflow payloads must be shared between pods. Set one of:

- **`storage.url`**: an `s3://` or `az://` bucket. Credentials go in a Secret named by `storage.optionsSecret`, with keys such as `HUXLEY_STORAGE__OPTIONS__ACCESS_KEY_ID`.
- **`storage.persistence.enabled`**: a ReadWriteMany volume mounted by every Huxley pod.

## Worker pools

The workflow engine has three kinds of job:

- **`step`** runs one step. It's often a call to the outside world: HTTP, AI, scripts.
- **`advance`** walks a run forward. It's quick bookkeeping.
- **`deadline`** enforces a run's timeout.

The chart starts two pools by default. **steps** takes `step` jobs. **runs** takes `advance` and `deadline` jobs. Slow integrations then can't hold up runs moving forward, and each pool scales on its own. Each pool sets how many jobs a process runs at once (`concurrency`). A pool with no kinds listed takes every kind.

```yaml
workers:
  pools:
    - { name: steps, kinds: [step], concurrency: 16, replicas: 2, minReplicas: 1, maxReplicas: 20 }
    - { name: runs, kinds: [advance, deadline], concurrency: 16, replicas: 1, minReplicas: 1, maxReplicas: 6 }
```

Outside the chart, the same setting is `HUXLEY_ENGINE__KINDS=[step]` (or `[engine] kinds` in `huxley.toml`).

### Autoscaling with KEDA

With [KEDA](https://keda.sh) installed, `keda.enabled: true` scales each pool between `minReplicas` and `maxReplicas` on its backlog: the pool's due and running jobs, counted in PostgreSQL. The target is one process per `concurrency` jobs. Pools never scale to zero, so timers and schedules keep running. KEDA reads PostgreSQL with the database user and password from `database.existingSecret`.

## PgBouncer

Every Huxley process keeps up to `database.maxConnections` connections, 20 by default. Many pods can outgrow PostgreSQL's connection limit. `pgbouncer.enabled: true` puts PgBouncer in transaction mode in between, so many client connections share a few server connections (`pgbouncer.poolSize` per replica).

Two things don't go through it:

- **LISTEN connections** (real-time updates and waking workers), **migrations** and **admin commands** go straight to PostgreSQL. The chart sets `database.direct_url` for them.
- **Session settings.** Huxley runs as the restricted `huxley_app` role under row-level security. Behind transaction pooling, PgBouncer switches role on each server connection itself (`connect_query`), and applies the statement timeout. With `database.pooled` set (the chart sets it), Huxley checks at start that its connections really run as `huxley_app`, and refuses to start if they don't.

To use your own pooler (RDS Proxy, Azure's built-in PgBouncer, or PgBouncer you run yourself), set `HUXLEY_DATABASE__URL` to the pooler, `HUXLEY_DATABASE__DIRECT_URL` to PostgreSQL, and `HUXLEY_DATABASE__POOLED=true`. Then make the pooler run `SET ROLE huxley_app` and set the statement timeout on its server connections. The pooler needs PgBouncer 1.21 or later, or another pooler that supports prepared statements in transaction mode.

## Read replicas

`database.readReplicaHost` sends reporting queries (dashboards, analytics) to a replica, using the same user and password.

## Upgrading

`helm upgrade` runs migrations first, in the Job, then rolls the Deployments. Migrations are written to be safe while the previous version still runs.
