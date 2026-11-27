# ADR-090: Configurable client address behind CDNs; rate limits shared in PostgreSQL

- **Status:** Accepted. The user asked for CDN support and chose PostgreSQL over Valkey for shared rate limits until large deployments show a need.
- **Date:** 2026-11-27

## Context

**The client address.** Huxley records the client's address in the audit log and keys the sign-in, public form and OAuth rate limits on it. With `trust_forwarded_for`, it took the rightmost `X-Forwarded-For` entry: the address the nearest proxy saw. That's right with one proxy (Caddy, an ingress). Behind a CDN that address is the CDN's edge server, so:

- many users share a few addresses and trip each other's rate limits;
- the audit log records the CDN instead of the user.

The web and portal apps had the same problem, with `XFF_DEPTH=1`.

**Rate limits.** The limiters were per process, in memory. With several API replicas, each enforced its own window, so the effective limit grew with the number of replicas. The design (ADR-002) left shared limits to Valkey when deployed. Nothing else has needed Valkey: the queue, worker wakeups and live updates all run on PostgreSQL.

## Decision

**The client address can come from:**
- **`http.client_ip_header`:** a header the CDN sets to the client's address (`CF-Connecting-IP`, `X-Azure-ClientIP`, `True-Client-IP`). It's used first when present and valid. This works whatever the proxies in between do to `X-Forwarded-For`, which matters because Caddy and ingress-nginx replace it for untrusted senders by default.
- **`http.proxy_hops`:** the number of proxies that append to `X-Forwarded-For`. The client is that many entries from the right; with fewer entries, it's the leftmost. That covers the web apps, which send only the client address they resolved.

Both apply only with `trust_forwarded_for`. Both are only as trustworthy as the path the request came through, so the docs make locking the origin to the CDN step one. The web and portal apps get the same choice through `ADDRESS_HEADER`/`XFF_DEPTH`, set together in Compose (`HUXLEY_CLIENT_IP_HEADER`) and Helm (`clientIp`).

**Rate limits move to PostgreSQL:**
- An unlogged `rate_limit_hits` table holds fixed-window counters keyed by scope and address, updated by one upsert per check.
- Every replica sees the same counts.
- If the database can't be reached, the check lets the request through and logs a warning. Requests that need the database fail anyway.
- The retention sweep deletes counters more than an hour old.

**Valkey comes out of the Compose and dev stacks**, where nothing used it. ADR-002's design still allows adding it, for wakeups, fan-out or limits, if real large deployments show PostgreSQL is the bottleneck.

## Consequences

- Each limited request costs one small upsert. These are rare requests: sign-ins, public forms, OAuth.
- One address hammering an endpoint contends on one row. That's acceptable: it's exactly the traffic being limited.
- Counters are lost in a database crash, since the table is unlogged. Limits then start again, which is harmless.
- "Behind a CDN" in the operations docs covers the client address, origin lock, caching, timeouts (synchronous webhooks against CDN limits) and WAF exceptions.
