# ADR-016: TypeScript SDK compiles to HWF at build time

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Some teams prefer real code to YAML. A second runtime would mean a second execution path and security model.

## Decision

`@usehuxley/sdk` compiles TypeScript to HWF with `huxley-cli build`. The engine only ever executes HWF. The SDK ships in Phase 7, after HWF v1 stabilizes.

## Consequences

One execution path. SDK workflows open on the canvas. The SDK must track HWF schema changes, which is why it lives in the monorepo.
