# Contributing to Huxley

Thanks for helping. This guide covers how work is organized and what a change needs before it merges.

## Before you start

- Read the [design document](docs/DESIGN_DOCUMENT.md) and the relevant [ADRs](docs/adr/). Significant changes to architecture need a new ADR.
- For anything larger than a small fix, open an issue first so the approach can be agreed before you invest time.

## Setup

See the Development section of the [README](README.md). Enable the git hooks once per clone:

```sh
pnpm exec lefthook install
```

## Making a change

- Branch from `main`. Keep pull requests focused on one change.
- Follow the [API conventions](docs/api-conventions.md) for any endpoint.
- If you change the API, run `just api-client` and commit the regenerated client.
- New SQL migrations must be expand-then-contract compatible so rolling deploys work: add before you remove, and never edit a migration that has merged.
- Run `just check` before pushing.

## Definition of done

From the [implementation plan](docs/IMPLEMENTATION_PLAN.md):

- Tests cover the change: unit tests, plus integration tests against real PostgreSQL for data access and e2e tests for user flows.
- Endpoints and actions enforce authorization; list endpoints filter by it.
- Configuration and entity changes write audit events (from Phase 1).
- New background work emits OpenTelemetry spans and metrics.
- UI uses `@usehuxley/ui`, works with the keyboard, in light and dark themes, and at phone width.
- User-facing behavior is documented.

## Integrations and packs

Integrations and automations are the easiest way to help: they ship as packs (ADR-028).

- **Built-in packs** live in `crates/huxley-packs/builtin/<name>/` (a `pack.yaml` and its content folders) and are listed in `crates/huxley-packs/src/builtin.rs`. Huxley ships them inside the binary, trusted by construction. Built-in packs are for integrations most MSPs use and for the automations Huxley's own modules need.
- **An integration** is an OpenAPI document, with an overlay for what the vendor's document gets wrong or leaves out: auth schemes, pagination, friendly names. Start from the vendor's published spec rather than writing one, and keep the overlay small. Add a test that imports it, against a mock of the vendor's API: see `crates/huxley-engine/tests/` for examples.
- **Workflows** in a pack must validate (`huxley workflows validate`), and must use the integration's operations rather than raw HTTP where one exists.
- Read [Writing packs](docs/pack-authoring.md) first: what to include, what never to include (credentials, customer data), and how versions and upgrades work.
- A change to a built-in pack bumps its `version` (semver), so instances see the upgrade and review it.

Packs that only some MSPs need are better published as signed packs, from your own registry (`huxley pack index`, see [Packs](docs/packs.md#hosting-a-registry)), than added as built-ins.

## Dependencies

- Rust and npm dependencies must use permissive, MIT-compatible licenses (ADR-012). `cargo deny check` enforces this for Rust.
- pnpm refuses packages younger than its minimum release age and blocks install scripts unless allowed in `pnpm-workspace.yaml`. Don't bypass either without a reviewed reason.
- GitHub Actions are pinned to commit SHAs. Renovate keeps them current.

## Commits

Write commit messages in the imperative mood ("Add queue reaper") with a short summary line. Explain *why* in the body when it isn't obvious.

## Code of conduct

Participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).
