# ADR-014: Huxley generates invoices; accounting systems are the system of record

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Rebuilding AR, payments, and tax compliance is out of scope.

## Decision

Huxley generates invoices from time, agreements, projects, and sales, then syncs them to QuickBooks Online, Xero, and others through integration packs. Payment status syncs back. Huxley does not process payments.

## Consequences

A smaller billing module. Accounting integrations are critical paths and need strong mapping and error handling.
