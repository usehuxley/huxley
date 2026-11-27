# ADR-089: A Helm chart with worker pools by job class, KEDA, and PgBouncer

- **Status:** Accepted (implementer's design). Implements the Helm chart, worker pools by job class, KEDA autoscaling and PgBouncer from Phase 13 (plan §3).
- **Date:** 2026-11-27

## Context

Larger deployments run on Kubernetes and need to scale work independently. One process type running every role, as the Compose deployment does, doesn't fit that. The engine's jobs differ:

- `step` jobs call the outside world and can take seconds or minutes.
- `advance` and `deadline` jobs are quick bookkeeping.

When both kinds share workers, a burst of slow steps holds up every run's progress.

Each process also holds a connection pool, so many pods can exhaust PostgreSQL's connections. PgBouncer in transaction mode is the usual answer, but Huxley depended on session state:

- `SET ROLE huxley_app` ran once per connection, after connecting. Row-level security depends on that role.
- The statement timeout was sent as a startup parameter.
- LISTEN needs a session of its own.

## Decision

**Worker pools by job class.**
- `engine.kinds` (`HUXLEY_ENGINE__KINDS`) limits which job kinds a process's workers claim. Empty means all, as before.
- Unknown kinds stop the server at start.
- The claim query filters on `kind`. The existing due-jobs index still serves it, since the pools split a small set of kinds.

**Pooling-safe database settings.**
- `database.direct_url` is a connection straight to PostgreSQL. LISTEN connections, migrations and admin commands (owner pools) use it. It defaults to `url`.
- `database.pooled` says `url` goes through a transaction pooler.
  - Huxley stops sending the startup parameter, which PgBouncer would refuse.
  - At start, Huxley checks that a pooled connection's `current_user` is `huxley_app` and refuses to start otherwise. A pooler that didn't switch role would otherwise run queries as the login role and skip row security.
  - The pooler must run `SET ROLE huxley_app` and set the statement timeout on each server connection. PgBouncer does this with `connect_query`.
- All other session state Huxley uses was already transaction-scoped: `set_config(…, true)`, `SET TRANSACTION`, `pg_advisory_xact_lock`.

**The Helm chart** (`deploy/helm/huxley`):
- **Huxley processes:**
  - the API, which also runs the runner gateway;
  - one Deployment per worker pool (by default `steps` and `runs`);
  - the scheduler;
  - web and portal;
  - a migration Job as a pre-install/pre-upgrade hook.
- **Optional parts:**
  - PgBouncer (1.21+ for prepared statements) with `connect_query`;
  - KEDA `ScaledObject`s, one per pool;
  - an HPA on CPU for the API;
  - a read replica.
- **Ingress:** routes the same paths the Compose Caddyfile does.
- **Pod defaults:** non-root, a read-only root filesystem, all capabilities dropped, and PodDisruptionBudgets.
- **Secrets:** taken from existing Secrets (preferred), or created as hooks kept on uninstall.
- The database URL is composed from its parts, with the password from a Secret through `$(DB_PASSWORD)`.
- The chart refuses to render without a database, a KEK, and shared storage: an object store, or a ReadWriteMany volume.

**KEDA's trigger** is a PostgreSQL query counting a pool's due and running jobs. The target is the pool's per-process `concurrency`. `minReplicas` is at least 1, since timers are polled. No metrics stack is needed.

**CI** runs `helm lint` and `helm template` on two value sets (everything on, and minimal), and validates the output with kubeconform, including KEDA's CRDs. It also checks that the chart refuses to render with default values.

## Consequences

- The full e2e suite passes with Huxley behind PgBouncer in transaction mode, migrations and admin commands going direct.
- KEDA's query reads the `jobs` table directly. A schema change there has to update the chart's query. It's in one template.
- Worker and scheduler pods have no HTTP probes. They have no HTTP server, and Kubernetes restarts them if they exit.
- No images or charts are published yet. Installing means building the images, and the docs say so.
