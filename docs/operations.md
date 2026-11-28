# Running Huxley

Notes for whoever runs a Huxley instance.

## Data retention

Some data grows without end, so a daily sweep removes what's older than its retention (ADR-085). Set the retentions in **Settings → Data retention** (`platform:retention:manage`).

| What | Kept for (default) | Notes |
| --- | --- | --- |
| Finished workflow runs | 12 months | With their step details and any payloads stored in object storage. Runs still running or waiting are never removed. |
| Delivered domain events | 30 days | Events workflows haven't picked up yet stay. |
| Read notifications | 180 days | Unread ones stay. |
| AI call records | 365 days | Usage and cost history. |

The sweep runs in the scheduler role a few minutes after start, then daily. **Sweep now** runs it straight away, and the page shows what the last sweep removed. Run partitions the sweep empties are dropped.

The **audit log** isn't swept. It's a hash chain (`huxley admin verify-audit` checks it), so export it to your SIEM or archive instead of deleting it.

## The audit log

Every change is recorded in the audit log, which is a hash chain. Each record carries the hash of the one before it, so an edit or a deletion breaks the chain. `huxley admin verify-audit` checks it.

Records are chained ("sealed") about a second after their change commits, by the scheduler role (ADR-086). The audit list and verification seal anything waiting first.

### Streaming it to a SIEM

Settings → Audit streaming sends audit records to your SIEM as they're sealed, in order. You need `platform:audit_streams:manage`. There are two kinds of stream:

- **Webhook:** NDJSON, one record per line, POSTed to your URL. A secret goes in the header you choose (`Authorization` by default). For syslog, Elastic, Microsoft Sentinel, Datadog and others, point a webhook at a forwarder such as [Vector](https://vector.dev) or [Fluent Bit](https://fluentbit.io).
- **Splunk HEC:** the collector's URL (for example `https://splunk.example.com:8088`) and an HEC token. Events arrive with sourcetype `huxley:audit`.

Each record is the same as in an export: the actor, action, organization, target, details, its sequence number, the previous record's hash, and its own hash. Your SIEM can check the chain independently.

A new stream sends what's recorded from then on, unless you tick the option to send the whole audit log. If the destination fails, the stream keeps its place, backs off (up to 10 minutes), and catches up when the destination is back. The settings page shows the error and how many records are waiting. Delivery is at least once, so deduplicate on `id` if your SIEM needs to.

Streams go out through the same egress policy as workflow steps. A destination on a private network has to be allowed with `engine.egress.private_hosts` or `allow_private`.

## Behind a CDN

Huxley can sit behind Cloudflare, Azure Front Door, or a similar CDN or WAF, in front of Caddy (Compose) or your ingress (Kubernetes). Five things need setting.

### 1. Lock the origin to the CDN

Only the CDN should be able to reach Huxley. Otherwise anyone can go around it, and around its WAF, and claim any client address.

- **Cloudflare:** run Cloudflare Tunnel, or turn on Authenticated Origin Pulls and allow only Cloudflare's IP ranges.
- **Azure Front Door:** use Private Link to the origin, or allow only the `AzureFrontDoor.Backend` service tag and check that the `X-Azure-FDID` header carries your Front Door's ID.

### 2. Tell Huxley where the client's address is

Sign-in rate limits and the audit log record the client's address. Behind a CDN, the nearest proxy reports the CDN's edge server, so set one of these:

- **The CDN's header (simplest):** `HUXLEY_HTTP__CLIENT_IP_HEADER` set to `CF-Connecting-IP` (Cloudflare), `X-Azure-ClientIP` (Front Door) or `True-Client-IP` (Akamai and others).
  - **Compose:** set `HUXLEY_CLIENT_IP_HEADER` in `.env`.
  - **Helm:** set `clientIp.header`.
  - This works whatever your proxy does with `X-Forwarded-For`.
- **Hop counting:** `HUXLEY_HTTP__PROXY_HOPS=2`, so the client is two entries from the right of `X-Forwarded-For` (CDN edge, then your proxy). In Helm, set `clientIp.proxyHops`. This only works if your proxy appends to the `X-Forwarded-For` it receives, rather than replacing it: Caddy needs `trusted_proxies`, and ingress-nginx needs `use-forwarded-headers`.

Either way, `HUXLEY_HTTP__TRUST_FORWARDED_FOR` must be on (it is in Compose and Helm). Both rely on step 1: a header is only as trustworthy as the path it came through.

### 3. Don't cache what's dynamic

Bypass the cache for `/api/*`, `/hooks/*`, `/scim/*`, `/mcp*`, `/callbacks/*`, `/oauth/*` and `/.well-known/oauth-*`, and for every page of the staff app and portal: they're per user. Static assets under `/_app/immutable/` can be cached for as long as you like, since their names change with every build.

### 4. Mind the timeouts

| | Origin response timeout |
|---|---|
| Cloudflare | 100 seconds (Enterprise: configurable) |
| Azure Front Door | 60 seconds by default, up to 240 |

Live updates (server-sent events) send a keep-alive every 15 seconds, so they stay open. **Synchronous webhooks** (`response: sync`) can wait up to 5 minutes for a run. Behind a CDN, keep their `timeout` under its limit, or use `async`.

Uploads go up to `storage.max_upload_mb` (25 MB by default), within both CDNs' limits. To check them for malware, see [Malware scanning](attachment-scanning.md).

### 5. Let the WAF through where it must

Managed WAF rules sometimes block legitimate traffic. Add exceptions as needed:

- `/hooks/*`: webhook payloads from other systems, often JSON with HTML or SQL-looking text.
- `/scim/*`: your identity provider.
- `/mcp`: AI clients.
- `/api/v1/integrations` imports: OpenAPI documents.

Don't put the WAF's bot challenges on these paths: the callers are machines.

The script runner gateway (port 8443, mutual TLS) is internal. Never route it through the CDN.

## Rate limits

Sign-in attempts (`HUXLEY_HTTP__LOGIN_ATTEMPTS_PER_MINUTE`, 20 by default), public form submissions and OAuth client requests are limited per client address each minute. The counters live in PostgreSQL, so every API replica enforces the same limit (ADR-090). Accounts are also locked after repeated failed sign-ins, whatever the address.

## Protecting secrets

Secrets Huxley stores (connection credentials, MFA secrets, vault entries, AI keys, SSO client secrets) are encrypted with per-organization data keys. The data keys are encrypted ("wrapped") by a **key-encryption key** (KEK) (ADR-018). The KEK can be one of:

| Where | Settings | Notes |
|---|---|---|
| **Configuration** | `HUXLEY_CRYPTO__KEK` (or `_FILE`): 32 random bytes, base64. Make one with `huxley admin generate-kek`. | The simplest. Keep it in your secret store, and back it up with the database. |
| **HashiCorp Vault / OpenBao** | `HUXLEY_CRYPTO__VAULT__ADDRESS`, `__MOUNT` (default `transit`), `__KEY` (default `huxley`), plus a token, AppRole or Kubernetes auth. | Transit with an `aes256-gcm96` key. Runs anywhere, including on-premises. |
| **Azure Key Vault** | `HUXLEY_CRYPTO__AZURE_KEY_VAULT__VAULT_URL`, `__KEY` (an RSA key), plus a client secret, workload identity or managed identity. | Signs in like the Azure SDK. |
| **AWS KMS** | `HUXLEY_CRYPTO__AWS_KMS__KEY_ID` (key id, ARN or `alias/…`), `__REGION` (or `AWS_REGION`). | A symmetric KMS key. Credentials come from the AWS default chain (ECS task role, EKS Pod Identity or IRSA, EC2 instance role), or `__ACCESS_KEY_ID` and `__SECRET_ACCESS_KEY` outside AWS. |

With a key service, the KEK never leaves it: Huxley asks it to wrap and unwrap data keys. Each wrap is tied to its data key and organization, so a wrapped key copied to another row is refused.

**What an AWS role needs:** `kms:Encrypt` and `kms:Decrypt` on the key. Every request carries the encryption context `huxley=data-key`, so you can require it in the key policy (`kms:EncryptionContext:huxley`), and CloudTrail shows it. Turn on automatic key rotation; old wraps keep decrypting.

### Changing where the KEK lives

1. Configure both the old KEK and the new one, and set `HUXLEY_CRYPTO__ACTIVE` to the new one (`kek`, `vault`, `azure_key_vault` or `aws_kms`).
2. Run `huxley admin rewrap-keys`. It re-wraps every data key with the active KEK. The secrets themselves aren't re-encrypted.
3. Remove the old KEK.

To retire a configured KEK, list it in `HUXLEY_CRYPTO__RETIRED_KEKS` while you rewrap.

