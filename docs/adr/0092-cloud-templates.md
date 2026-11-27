# ADR-092: Terraform templates for AWS, Azure and DigitalOcean; AWS KMS for the key-encryption key

- **Status:** Accepted. The plan had DigitalOcean and Azure templates; the user added AWS and approved Terraform for all three.
- **Date:** 2026-11-27

## Context

Phase 13 calls for deployment templates for DigitalOcean and Azure (plan §3). AWS is the largest cloud and common among MSPs. Three things in Huxley stood in the way of a clean AWS deployment:

- The key-encryption key could live in Vault or Azure Key Vault, but not in AWS KMS.
- Storage built its object store from explicit options only, so S3 on ECS couldn't use the task role.
- CloudFront's client-address header carries a port.

## Decision

- **Terraform for all three clouds**, rather than Bicep for Azure. One tool and one layout, validated in CI (`fmt -check` and `validate`). The templates are self-contained, with no third-party modules.
- **AWS:** ECS Fargate, RDS (password managed in Secrets Manager), S3, and a KMS key restricted to the task role and Huxley's encryption context. An ALB routes as the Caddyfile does, with optional CloudFront and an origin-lock header. Internal calls use ECS Service Connect, with the port as plain TCP so `X-Forwarded-For` isn't rewritten. EKS users take the Helm chart.
- **Azure:** Container Apps on a VNet, PostgreSQL Flexible Server with private access, Blob Storage with shared keys off, and an HSM-backed Key Vault RSA key, all through a user-assigned managed identity.
  - A small Caddy router is the only public ingress, since Container Apps can't route by path between apps. It overwrites `X-Huxley-Client-IP` with the client address it derives from the ingress's `X-Forwarded-For`, trusting private ranges. Huxley and the web apps read that header, so hops added inside the environment don't matter. The web apps forward it on their API calls too.
  - Worker pools scale with Container Apps' KEDA PostgreSQL scaler.
- **DigitalOcean:**
  - App Platform with Managed PostgreSQL (app-only firewall), Spaces with a bucket-scoped key, and `DO-Connecting-IP` for the client address. DigitalOcean has no key management service, so the KEK is a generated, encrypted app secret.
  - Alternatively, one Droplet runs the Compose deployment via cloud-init, for small installs.
  - Roles are set with `HUXLEY_ROLES`, because App Platform image commands may run through a shell and the image is distroless. The image's default command is now `serve` (roles default to all), so the variable can take effect.
- **Huxley changes:**
  - `crypto.aws_kms` uses the official AWS SDK for the credential chain (ECS, EKS Pod Identity and IRSA, EC2 roles), with the same rustls/aws-lc stack. See ADR-018.
  - Storage folds the provider's environment (`AWS_*`, `AZURE_*`, `IDENTITY_ENDPOINT`) under its explicit options, as `object_store`'s `from_env` would.
  - `client_ip_header` accepts `address:port` values, and always strips the port from `CloudFront-Viewer-Address`.
- **Database passwords** reach Huxley as `PGPASSWORD`, with a password-less URL. sqlx reads `PG*` variables first and lets the URL override them, so no cloud has to splice a secret into a URL.
- **Migrations run at start** (`migrate_on_start`) on these platforms, which have no hook step. sqlx's migration lock makes concurrent starts take turns.

## Consequences

- The templates are validated, not applied in CI, since that would need cloud accounts and costs money. The Azure router's Caddyfile was run against stand-in upstreams to check routing and the client header. The KMS provider was tested against a mock of KMS's API.
- Two edge details aren't verified against the real clouds: App Platform's `DO-Connecting-IP` header, and Container Apps' internal hops. In both cases a wrong guess only falls back to a proxy's address in rate limits and audit records; it doesn't break anything.
- Until images are published, every template needs images built and pushed first, and the Droplet builds from source.
