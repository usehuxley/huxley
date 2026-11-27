# ADR-087: The audit log is streamed to SIEMs over HTTPS, in chain order

- **Status:** Accepted (implementer's design). Implements "SIEM audit streaming" from Phase 13 (plan §3).
- **Date:** 2026-11-27

## Context

Security teams want Huxley's audit log in their SIEM next to everything else. They want it as it happens, not as a periodic export. The log is a hash chain (ADR-086 seals records shortly after commit), so streamed records can carry what's needed to check the chain on the other side.

SIEMs take data in many ways: syslog, Splunk HEC, Elastic's bulk API, Sentinel's ingestion API, Datadog's log intake and others. Most deployments already run a forwarder, such as Vector, Fluent Bit or Cribl, that takes HTTP in and speaks everything out.

## Decision

- **Streams are instance-wide settings** (`platform:audit_streams:manage`), at Settings → Audit streaming. There can be any number of them.
- **Two kinds:**
  - **Webhook:** records POSTed as NDJSON, one record per line. An optional secret goes in a header of the user's choice, `Authorization` by default.
  - **Splunk HEC:** records POSTed as HEC events (`time`, `source: huxley`, `sourcetype: huxley:audit`, `event`), with `Authorization: Splunk <token>`. A collector URL without a path gets `/services/collector/event`.

  Anything else goes through a forwarder pointed at a webhook. Native syslog/TLS and vendor APIs can come later if people ask.
- **Records are the export's records** (`seq`, id, time, actor, via, IP, action, org, target, details, `prevHash`, `hash`). A SIEM can verify the chain with the same rules as `huxley admin verify-audit`.
- **Only sealed records are shipped, in `seq` order.** Each stream keeps `last_seq`, the last record its destination accepted.
  - A new stream starts from the current chain head, or from the beginning if asked to ship the whole log.
  - Batches are 500 records. A pass ships up to 20 batches per stream.
- **Failures:**
  - Any non-2xx response or transport error keeps the stream's place and records the error and the count of failures in a row.
  - The stream backs off exponentially, from 5 seconds up to 10 minutes.
  - Saving a stream tries it again straight away.
- **The scheduler role ships every 5 seconds.** A stream is locked with `FOR UPDATE SKIP LOCKED` while it's shipped, so several schedulers never send the same batch twice.
- **Outbound requests go through the engine's egress policy**, the same SSRF protection workflow steps get. The secret is encrypted, bound to the stream, and never returned.
- **A test button** sends one marked test event. It isn't part of the audit log and doesn't move the stream's place.
- **Adding, changing and removing streams are audited.**

## Consequences

- **Delivery is at least once.** If a destination accepts a batch but the cursor update fails (the database is lost mid-commit), that batch is sent again. Records carry `seq` and `id`, so SIEMs can deduplicate.
- **Records reach the SIEM about 1 to 6 seconds after commit:** sealing every second, plus shipping every 5 seconds.
- A destination that's down for a long time just falls behind. Its place is kept, and the backlog is shipped once it recovers. The settings page shows how far behind each stream is.
- Retention never prunes the audit log (ADR-085), so a stream can't lose records it hasn't shipped.
