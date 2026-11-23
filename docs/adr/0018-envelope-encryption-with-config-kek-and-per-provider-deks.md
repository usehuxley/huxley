# ADR-018: Envelope encryption with a configured KEK and per-provider-org data keys

- **Status:** Accepted
- **Date:** 2026-11-22

## Context

TOTP secrets, OIDC client secrets, connection credentials, and vault items must be encrypted at rest (design doc §12). Each MSP brand under a common owner should have separate keys. Operators need to rotate the master key without re-encrypting every value, and a misconfigured key must not silently corrupt or lose data.

## Decision

- `huxley-crypto` implements envelope encryption with AES-256-GCM (`ring`, already in the dependency tree via rustls).
- The **key-encryption key** (KEK) comes from configuration (`crypto.kek`, usually `HUXLEY_CRYPTO__KEK_FILE`). It is identified by a domain-separated SHA-256 fingerprint, so no key names need managing. Retired KEKs stay configured until `huxley admin rewrap-keys` re-wraps every data key under the active one. KMS-backed KEKs plug in behind the same key ring (see below).
- Each **provider org** gets a data-encryption key (DEK), created on first use, stored only wrapped in `data_keys`. Values for a client org use its nearest provider ancestor's key. The wrap is bound to the DEK id and org id.
- Every ciphertext names its DEK and is bound (GCM associated data) to a purpose string and the owning record's id, so it cannot be moved to another row or reused for another field.
- The server refuses to start if any stored DEK can't be unwrapped with the configured KEKs. The app role cannot delete `data_keys` rows.
- A KEK is required to run the server. The compose installer generates one; development and CI use fixed throwaway keys.

### KEKs in a key management service (2026-11-23)

- The key ring holds KEKs of different kinds behind one async interface: a configured key, or a key in a KMS that Huxley asks to wrap and unwrap data keys. HashiCorp Vault and OpenBao (Transit) come first because they run anywhere, including on-premises and on DigitalOcean, and can be tested in CI; Azure Key Vault and AWS KMS follow behind the same interface.
- What a KMS encrypts is `huxley-dek:v1: || dek_id || org_id || dek`, checked after decryption. This binds the wrap to its row for every provider, including those whose wrap operation takes no associated data (Azure Key Vault's RSA wrap).
- A KMS KEK's id is `<provider>:<path>` (e.g. `vault:transit/huxley`), not a fingerprint. Key versions inside the KMS are the KMS's business; `rewrap-keys --all` re-wraps under the newest version.
- When more than one KEK is configured, `crypto.active` says which wraps new keys; the others stay readable, so moving from a configured KEK to a KMS is the same rewrap as a rotation.
- At startup the server wraps and unwraps a probe with the active KEK and unwraps (and caches) every stored data key, so a KMS outage or bad credentials fail fast. The KMS is called once per data key per process, not per request.

## Consequences

KEK rotation is cheap (re-wrap a handful of rows). DEK rotation, which needs every encrypted column re-encrypted, is not implemented yet; it needs a registry of encrypted fields, which will come with connections and the vault. Losing the KEK loses all encrypted data, so backup guidance calls it out next to the database.
