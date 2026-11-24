# ADR-028: Packs are signed zips whose manifest lists every file's hash

- **Status:** Accepted (order and details chosen by the implementer at the maintainer's request)
- **Date:** 2026-11-23

## Context

Packs (design doc §6.6) bundle integrations, workflows, forms, scripts and templates so MSPs, Huxley and the community can share them. Installing one adds code that runs against customers' systems, so an admin must see exactly what a pack adds, and know whether it is what its publisher made.

## Decision

- **Format:** a `.hxpack` is a zip. At its root is `manifest.json`, which holds:
  - `key`, `name`, `version` (semver), `description`, `publisher`
  - **`files`: every other file's path with its SHA-256**
  - `contents`: the keys of what it adds

  Files sit in conventional folders:
  - `integrations/<key>.yaml`, the OpenAPI spec, plus an optional `<key>.overlay.json`
  - `workflows/<key>.yaml`, the HWF document
  - `forms/<key>.json`
  - `scripts/<key>.json` (name, language, parameters) plus `scripts/<key>.ps1` or `.py`
  - `templates/<key>.yaml`
- **Signature:** an optional `signature.json` holds an **Ed25519** signature over the **exact bytes** of `manifest.json`, with the publisher and key id. The manifest commits to every file's hash, so the signature covers the whole pack.
- **Reading is strict:**
  - at most 20 MB and 500 files
  - every path relative, inside a known folder, with no `..`
  - every file present with its hash, and no files the manifest doesn't list
- **Trust:**
  - *trusted*: signed by a key the instance trusts (Huxley's, or `packs.trusted_keys`)
  - *unknown signer*: a valid signature by a key it doesn't trust
  - *unsigned*
  - a broken signature fails the read
- **Installing:** every install shows the review: contents, egress domains, scripts. An admin approves it, and unsigned or unknown-signer packs say so prominently.
- **Keys:** the CLI makes, builds and signs them (`huxley pack keygen | build | inspect`). Public keys look like `hxpk_<base64url>`.

## Consequences

- Publishers sign with a key they keep. Rotating it means re-signing, and instances trusting the new key.
- Built-in packs ship inside the binary, so they're trusted by construction.
- A registry (design §6.6, "later") can serve these files unchanged.
