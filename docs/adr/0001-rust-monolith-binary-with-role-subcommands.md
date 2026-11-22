# ADR-001: Rust monolith binary with role subcommands

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Huxley must be simple to run for a one-technician MSP and scale for large ones. Separate microservices would add deployment, networking, and versioning overhead that small installs can't absorb.

## Decision

Ship one Rust binary, `huxley`, with role subcommands: `serve --roles api|worker|scheduler|all`, `migrate`, `openapi`, `healthcheck`. Script runners are a separate image because they are a security boundary (ADR-007).

## Consequences

Small installs run one process. Large installs run each role as its own deployment and scale it independently. All roles share one codebase and one release version, so there is no cross-service API versioning.
