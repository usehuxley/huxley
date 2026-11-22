# ADR-005: minijinja for expressions

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

The overview requires Jinja for data manipulation. Expressions run on untrusted user input inside the engine.

## Decision

Use `minijinja`: Jinja2-compatible, written in Rust, sandboxed, and extensible with custom filters. Expressions of the form `{{ expr }}` evaluate to typed values. Secrets are taint-tracked and redacted from logs.

## Consequences

No embedded Python or JavaScript in the core. Users get familiar syntax. Fuel and output limits bound evaluation cost.
