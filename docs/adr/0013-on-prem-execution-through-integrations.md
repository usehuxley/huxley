# ADR-013: On-prem execution through integrations

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Some automation must run inside customer networks (AD, on-prem Exchange, endpoints).

## Decision

Run that work through integrations that already have a presence there: RMM script execution, Intune remediations, Azure Arc Run Command. A first-party Huxley agent is a future possibility only. The runner protocol stays transport-agnostic so one could plug in.

## Consequences

No agent to build, secure, and distribute. Capability depends on the MSP's existing tools.
