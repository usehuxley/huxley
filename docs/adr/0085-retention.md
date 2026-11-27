# ADR-085: A daily sweep enforces retention; the audit log is exported, not pruned

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 13 asks for retention jobs and partition maintenance automation. Workflow runs and their steps are partitioned by month, on UUIDv7 ranges, and a worker already creates partitions ahead. Nothing removed old data: runs, spilled payloads in object storage, published events, read notifications and AI call records grew without end.

## Decision

- **One instance-wide settings row**, with:
  - run retention in months (default 12);
  - days for published events (30), read notifications (180) and AI call records (365).
- **A daily sweep** in the scheduler role (first run five minutes after start), also available on demand:
  - **Runs:** finished runs older than the retention are deleted in batches of 500. Their steps and lookups cascade.
  - **Payloads:** first, each deleted run's payload prefix in object storage is removed. This needed a new `Storage::delete_prefix`.
  - **Partitions:** the sweep then drops run partitions that lie wholly before the cutoff and are empty, through a `SECURITY DEFINER` function like the one that creates them.
  - **Unfinished runs** are never removed, however old.
  - **Events, notifications and AI calls:** published events, read notifications and AI call records past their days are deleted. Events get a new delete policy that only allows published rows in system scope.
  - The last sweep's time and counts are kept for the settings page.
- **The audit log is excluded.** It's a tamper-evident hash chain. Pruning it would break verification, so it's streamed or exported (SIEM streaming is a later Phase 13 item).

## Consequences

- Deleting by row (rather than detaching partitions) keeps foreign keys and cascades simple, at the cost of more I/O for very large instances. Dropping emptied partitions keeps the catalog tidy.
- Rows that reference runs with `SET NULL` (triage, for example) lose the link when the run goes. Their own data stays.
