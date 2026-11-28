# Cloud templates

`deploy/terraform` has a Terraform template for each of three clouds. Each one builds a production-shaped Huxley with that cloud's managed services:

| | Runs on | Database | Objects | Key-encryption key | In front |
|---|---|---|---|---|---|
| **AWS** (`aws/`) | ECS Fargate | RDS for PostgreSQL (Multi-AZ) | S3 | AWS KMS | Application Load Balancer, optionally CloudFront |
| **Azure** (`azure/`) | Container Apps | PostgreSQL Flexible Server (zone-redundant, private) | Blob Storage | Key Vault (HSM-backed) | A router app, then Container Apps ingress |
| **DigitalOcean** (`digitalocean/`) | App Platform, or one Droplet | Managed PostgreSQL | Spaces | An encrypted app secret | App Platform's edge |

On Kubernetes (EKS, AKS, DOKS), use the [Helm chart](./kubernetes.md) instead.

Every template:

- **Runs worker pools by job class.** A `steps` pool handles calls to the outside world, and a `runs` pool handles bookkeeping. On Azure each pool scales on its own backlog; elsewhere they have fixed counts.
- **Keeps secrets out of the configuration.** The database password comes from the cloud's secret store, and the key-encryption key from its key service where there is one.
- **Routes like the Compose Caddyfile.** The API, webhooks, SCIM, MCP and OAuth paths go to the API, the rest of the staff host to the web app, and the portal host to the portal.
- **Sets the client address correctly** for its edge, so rate limits and the audit log see real users (ADR-090).
- **Applies migrations at start.** Processes starting together take turns.

## Before you start

1. **Images.** No images are published yet. Build `huxley`, `web` and `portal` from `docker/`, and push them to the cloud's registry (ECR, ACR or DOCR).
2. **Host names.** Pick two: one for the staff app and API, one for the client portal.
3. **TLS.**
   - **AWS:** an ACM certificate covering both names; for CloudFront, one in us-east-1 as well.
   - **Azure:** add both names as custom domains with managed certificates on the router app, after the first apply.
   - **DigitalOcean:** App Platform issues certificates. The Droplet uses Caddy.

```sh
cd deploy/terraform/aws        # or azure, digitalocean
cp terraform.tfvars.example terraform.tfvars   # and edit
terraform init
terraform apply
```

Then point DNS where the `dns` output says. Once the API is healthy, run the command in the `bootstrap` output. It sets the instance up and prints the first user's password.

## AWS

- **Network:** a VPC across two availability zones. The load balancer sits in the public subnets; tasks and the database are in private subnets behind one NAT gateway. S3 traffic stays inside AWS.
- **Key-encryption key:** a KMS key with rotation on. Its policy lets only the task role use it, and only with Huxley's encryption context (`huxley=data-key`).
- **Database:** RDS manages the database password in Secrets Manager, and rotates it. ECS hands it to Huxley as `PGPASSWORD`.
- **Getting a shell:** `aws ecs execute-command` reaches the API task. The `bootstrap` output uses it.
- **CloudFront** (`cloudfront.enabled`):
  - It caches only built assets (`/_app/immutable/*`).
  - The load balancer then answers only requests that carry CloudFront's secret header, so nobody can go around the CDN.
  - The client address is read two hops back.

## Azure

- **The router:** a small Caddy app is the only public ingress. Container Apps can't route by path between apps, so the router does it, and sets the client address in a header it controls.
- **Database:** PostgreSQL is VNet-integrated with no public access, and allows the extensions Huxley needs.
- **Identity:** the apps use a user-assigned managed identity for Blob Storage (shared-key access is off), for wrapping keys with Key Vault, and for pulling from ACR.
- **Worker scaling:** worker pools scale with Container Apps' built-in KEDA PostgreSQL scaler, the same query as the Helm chart.
- **Custom domains:** add `host` and `portal_host` to the router app:

  ```sh
  az containerapp hostname add  -g <rg> -n <name>-router --hostname huxley.example.com
  az containerapp hostname bind -g <rg> -n <name>-router --hostname huxley.example.com \
    --environment cae-<name> --validation-method CNAME
  ```

  Create the CNAME first, plus the `asuid` TXT record with the `domain_verification_id` output.

## DigitalOcean

- **`shape = "app_platform"`** (the default):
  - App Platform runs the API, the worker pools, the scheduler, and the web and portal apps. It routes by host and path.
  - Managed PostgreSQL only accepts connections from the app.
  - Spaces holds objects, through a key limited to its bucket.
  - The client address comes from App Platform's `DO-Connecting-IP` header.
  - DigitalOcean has no key management service, so the key-encryption key is a generated, encrypted app secret. Back it up (`terraform output -raw kek`), or point `crypto.vault` at a Vault server.
- **`shape = "droplet"`:** one Ubuntu Droplet runs the [Compose deployment](https://github.com/usehuxley/huxley/tree/main/deploy/compose), with backups on and a firewall that opens 80 and 443 (and SSH from the addresses you list). Until images are published it builds from source, so it needs about 8 GB of memory. That's for small installs.

## After the first apply

- **Email:** set `smtp_url` (or `smtp_url_secret_arn` on AWS) to send mail.
- **Behind a CDN:** see [Behind a CDN](./operations.md#behind-a-cdn). The AWS template has CloudFront built in.
- **Telemetry:** set `otlp_endpoint` to send telemetry to a collector. See [Observability](./observability.md).
- **Key-encryption key:** the key is in the cloud's key service (or the DigitalOcean secret). Keep it with the database's backups, because encrypted data can't be recovered without it. See [Protecting secrets](./operations.md#protecting-secrets).

## From a marketplace image

Once Huxley is listed on a cloud's marketplace, a server made from its image starts with Docker, the Compose deployment and its images in place, and secrets unique to that server. Point DNS for the staff app and the portal at the server, sign in over SSH as root, and run `huxley-setup`. It asks for the domains, outgoing email and your first user, then starts Huxley. Back up `/opt/huxley/deploy/compose/secrets/kek` straight away ([Backup and restore](backup.md)).

The images are built from [`deploy/marketplace`](https://github.com/usehuxley/huxley/tree/main/deploy/marketplace).

