# ADR-012: MIT-compatible dependencies only

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Huxley is MIT licensed. Copyleft or source-available dependencies would compromise that.

## Decision

Allow only permissive licenses, enforced by cargo-deny (`deny.toml`) in CI. npm license checks are added alongside.

## Consequences

Some libraries are off-limits. Exceptions require a maintainer review and an update to this ADR.
