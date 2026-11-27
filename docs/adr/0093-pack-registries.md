# ADR-093: Pack registries are static, hash-pinned indexes, optionally signed

- **Status:** Accepted (implementer's design). Implements the pack registry and authoring docs from Phase 13 (plan §3). Publishing the community registry itself waits for the user.
- **Date:** 2026-11-27

## Context

The design plans a community pack registry backed by a public git repo and served at `packs.usehuxley.io` (§6.6, §17). Packs are signed, hash-listed zips (ADR-028), installed only after an admin reviews what they add. MSPs also want to share packs with their own clients without emailing files around. Creating the public registry is an outward-facing step for the user to take. Everything else can be built and tested now.

## Decision

- **A registry is static files:** `index.json`, listing every version of every pack with its download URL, SHA-256, size, signer and contents, next to the `.hxpack` files. Any static host can serve one: object storage, GitHub Pages, nginx. There's no server to run, and a git repo plus CI can publish one.
- **`huxley pack index <dir> --base-url … [--key …]`** builds the index. It checks every pack as an install would, and fails on duplicate versions. With a key, it signs the index's exact bytes as a detached `index.json.sig`, an Ed25519 signature in the same format packs use.
- **`packs.registries`** lists the indexes an instance browses, each with an optional `key`. With a key, an index that isn't signed by it is refused. That protects the list and its hashes from tampering in transit or on the host. Pack signatures still say who wrote each pack, and are separate from the registry key.
- **Reviewing from a registry** goes through the same review as an upload, with the same trust labels, conflicts, egress and scripts, permissions and audit. On top of that:
  - the download must match the index's SHA-256;
  - its manifest must name the requested key and version.

  A registry can't skip the review or upgrade anything silently.
- **Indexes are cached per process** for five minutes, capped at 5 MB, with packs at 20 MB.
- **Docs:**
  - registries and hosting one, in `docs/packs.md`;
  - a pack authoring guide, `docs/pack-authoring.md`;
  - integration and pack contribution guidelines, in `CONTRIBUTING.md`.

## Consequences

- MSPs can run private registries today. The community registry becomes a repo of signed packs, a CI job that runs `huxley pack index --key`, and a static host. When the user decides to publish it, instances add its URL and key, and a default entry can follow.
- CI builds, signs, indexes and serves a fixture registry with the CLI, and an e2e test reviews and installs from it through the UI. An API test covers versions ordered newest first, installed versions, a tampered download, and the wrong registry key.
- Registry fetches come from the server, not the browser, so outbound access to registry hosts is needed. They're admin-configured, so they don't go through the workflow egress policy, as with SSO and KMS.
