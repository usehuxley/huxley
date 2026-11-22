# ADR-002: PostgreSQL-backed job queue, Valkey optional

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

The workflow engine needs a durable queue. Brokers (RabbitMQ, NATS, Kafka) add infrastructure, and enqueueing outside the database transaction risks lost or phantom jobs.

## Decision

Use a `jobs` table claimed with `SELECT ... FOR UPDATE SKIP LOCKED`. Jobs are enqueued in the same transaction as the change that triggers them. Idle workers are woken with `LISTEN/NOTIFY`, or Valkey when deployed. The queue sits behind a trait so a broker can replace it later.

## Consequences

PostgreSQL is the only required dependency. Enqueueing is transactional. Throughput is bounded by PostgreSQL, which is ample for the target scale, and the trait leaves an exit.
