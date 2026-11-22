# ADR-007: Scripts run only in isolated runners

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

MSP automation depends on PowerShell and Python. Running user scripts inside the API or worker would expose the database and secrets.

## Decision

Scripts execute only in separate runner containers (pwsh, python, js) reached over gRPC with mTLS. Runners have no database access, run non-root with read-only filesystems and resource limits, and receive only the short-lived credentials a step needs.

## Consequences

A clear security boundary at the cost of an extra component. Runner pools scale independently.
