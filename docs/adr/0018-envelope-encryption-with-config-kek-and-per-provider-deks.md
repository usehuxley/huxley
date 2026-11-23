# ADR-018: Envelope encryption with a configured KEK and per-provider-org data keys

- **Status:** Accepted
- **Date:** 2026-11-22

## Context

TOTP secrets, OIDC client secrets, connection credentials, and vault items must be encrypted at rest (design doc §12). Each MSP brand under a common owner should have separate keys. Operators need to rotate the master key without re-encrypting every value, and a misconfigured key must not silently corrupt or lose data.

## Decision

- `huxley-crypto` implements envelope encryption with AES-256-GCM (`ring`, already in the dependency tree via rustls).
- The **key-encryption key** (KEK) comes from configuration (`crypto.kek`, usually `HUXLEY_CRYPTO__KEK_FILE`). It is identified by a domain-separated SHA-256 fingerprint, so no key names need managing. Retired KEKs stay configured until `huxley admin rewrap-keys` re-wraps every data key under the active one. KMS-backed KEKs (Azure Key Vault, HashiCorp Vault) plug in behind the same key ring later.
- Each **provider org** gets a data-encryption key (DEK), created on first use, stored only wrapped in `data_keys`. Values for a client org use its nearest provider ancestor's key. The wrap is bound to the DEK id and org id.
- Every ciphertext names its DEK and is bound (GCM associated data) to a purpose string and the owning record's id, so it cannot be moved to another row or reused for another field.
- The server refuses to start if any stored DEK can't be unwrapped with the configured KEKs. The app role cannot delete `data_keys` rows.
- A KEK is required to run the server. The compose installer generates one; development and CI use fixed throwaway keys.

## Consequences

KEK rotation is cheap (re-wrap a handful of rows). DEK rotation, which needs every encrypted column re-encrypted, is not implemented yet; it needs a registry of encrypted fields, which will come with connections and the vault. Losing the KEK loses all encrypted data, so backup guidance calls it out next to the database.
