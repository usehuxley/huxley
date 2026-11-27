# ADR-086: Audit records are sealed into the hash chain after commit

- **Status:** Accepted (implementer's design). Changes how the audit chain (Phase 0) is written.
- **Date:** 2026-11-27

## Context

The audit log is an append-only hash chain. Each record holds a sequence number, the previous record's hash and its own hash. Until now, a trigger chained each record as it was inserted, by updating the single `audit_chain_head` row. That held the row lock until the transaction committed, with two effects:

- **All audited writes were serialized**, platform-wide. Almost every change is audited, so this capped write throughput.
- **Deadlocks.** A transaction that wrote an audit record and then locked a row deadlocked with one that locked the row first and then wrote an audit record. Under parallel load (the e2e suite), sign-ins and invoice saves failed with "deadlock detected". That surfaced as intermittent failures that looked like timing.

## Decision

- **Records go in unsealed:** without `seq`, `prev_hash` or `hash`, stamped with `occurred_at`. Inserting takes no shared lock.
- **`huxley_audit_seal(batch)`** (`SECURITY DEFINER`) chains committed, unsealed records onto the head:
  - in `occurred_at` order, then by id;
  - each record gets the next `seq`, the head's hash as `prev_hash`, and its hash over the same canonical content as before;
  - the head moves on.
  - It takes the head with `FOR UPDATE SKIP LOCKED`, so one sealer works at a time and others return straight away.
- **Who seals** (on demand, a sealer waits for one already running, so what committed before is sealed when it returns; the background sealer skips instead):
  - every second, the scheduler role;
  - before listing, the audit list (only sealed records are listed);
  - before verifying, verification (`huxley admin verify-audit`).
- **Unchanged:**
  - The application role still can't update or delete audit rows. Only the sealer fills in the chain fields.
  - The hash, the chain and verification are the same.
  - Records sealed before this change keep their place.

## Consequences

- Audited transactions no longer contend with each other, which removes the deadlocks and the serialization point.
- There's a short window, normally under a second, in which a committed record isn't chained yet. It's still protected by the role's grants. A database owner could edit an unsealed record before it's sealed, but the same owner could already rewrite the chain, so the trust boundary doesn't change. SIEM streaming (Phase 13) can ship records as they're sealed.
- A record's `seq` reflects sealing order, which is commit order. It no longer reflects insert order. `occurred_at` is still the transaction's time.
