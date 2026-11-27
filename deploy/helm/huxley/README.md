# Huxley Helm chart

Huxley on Kubernetes: the API, worker pools by job class (with optional KEDA autoscaling), the scheduler, the web app and the client portal. Migrations run as a pre-install and pre-upgrade hook. PgBouncer is optional.

See [docs/kubernetes.md](../../../docs/kubernetes.md) for installing, storage, worker pools, KEDA and PgBouncer. `values.yaml` documents every setting.

```sh
helm install huxley deploy/helm/huxley -n huxley -f my-values.yaml
```

Required: `database.host`, `database.existingSecret` (or `database.password`), `kek.existingSecret` (or `kek.value`), and shared storage (`storage.url` or `storage.persistence.enabled`).
