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
