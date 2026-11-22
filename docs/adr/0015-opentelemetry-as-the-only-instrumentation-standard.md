# ADR-015: OpenTelemetry as the only instrumentation standard

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Operators run many different observability backends. Vendor SDKs would lock Huxley in.

## Decision

Instrument everything with OpenTelemetry and export traces, metrics, and logs over OTLP (`huxley-telemetry` crate). Propagate W3C trace context across API, queue, workers, runners, and outbound calls. Telemetry passes through the same redaction as run history.

## Consequences

Works with any OTLP backend, including Azure Monitor. Telemetry is off unless an endpoint is configured.
