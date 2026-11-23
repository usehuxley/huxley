# ADR-012: MIT-compatible dependencies only

- **Status:** Accepted
- **Date:** 2026-11-21

## Context

Huxley is MIT licensed. Copyleft or source-available dependencies would compromise that.

## Decision

Allow only permissive licenses, enforced by cargo-deny (`deny.toml`) in CI. npm license checks are added alongside.

## Consequences

Some libraries are off-limits. Exceptions require a maintainer review and an update to this ADR.

## Exceptions

| Crates | License | Approved | Reason |
|---|---|---|---|
| `webauthn-rs`, `webauthn-rs-core`, `webauthn-rs-proto`, `webauthn-attestation-ca`, `crypto-glue` (all from the kanidm webauthn-rs project) | MPL-2.0 | 2026-11-22 | Passkeys (design doc §11.2). The most mature Rust WebAuthn relying-party library, externally reviewed. MPL-2.0 is file-level copyleft: we use the crates unmodified, so Huxley stays MIT. Any patch to these crates' files must be published under MPL-2.0. |
| `quoted_printable` (dependency of `lettre`, email encoding) | 0BSD | 2026-11-22, confirmed by the maintainer 2026-11-23 | Zero-clause BSD: no conditions at all, more permissive than MIT. Scoped to this crate rather than added to the global allow list. |
