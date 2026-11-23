# ADR-023: Integration definitions come from OpenAPI through a tolerant importer

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

Phase 3 starts with OpenAPI import (design doc §7.1). An admin gives Huxley a vendor's spec, and Huxley produces an integration: base URLs, auth schemes, and one action per operation, each with an input schema the action step's form is generated from and an output schema expressions can be checked against.

Specs come in three formats: OpenAPI 3.0, OpenAPI 3.1 (JSON Schema 2020-12 semantics), and Swagger 2.0, which many MSP vendors still publish. Published specs are also often slightly wrong: a missing `info.version`, a duplicate `operationId`, a path placeholder that isn't declared, a `$ref` to a component that doesn't exist.

The typed Rust crates each cover part of this. `openapiv3` handles 3.0 only, `oas3` targets 3.1, and neither reads 2.0. Both fail a whole document on one structural mistake.

## Decision

The `huxley-integrations` crate imports specs with its own importer that works on the document as plain JSON:

- **Formats:** JSON or YAML (YAML 1.2, with the same parser and budgets as HWF). OpenAPI 3.0 and 3.1 are read directly. Swagger 2.0 is first converted to the 3.0 shape:
  - `host`/`basePath`/`schemes` become servers
  - `definitions` and `securityDefinitions` move under `components`
  - `body` and `formData` parameters become request bodies
  - response schemas go under the produced media type
- **`$ref`s:** local ones are inlined, so each action's schemas stand alone. A recursive schema is cut where it loops, and a node budget bounds how far shared schemas can multiply. External references aren't followed.
- **Mistakes:** problems that don't stop the API being usable become warnings with a JSON pointer, shown to the admin at import. Examples: a duplicate operation id gets a suffix, an undeclared path parameter is added as a string, and an unsupported auth type is skipped. Only an unreadable document, a non-OpenAPI document, or one over the size or operation limits fails the import.
- **The definition:**
  - title, version, servers with their variables
  - auth schemes: API key, HTTP basic or bearer, OAuth2 authorization code and client credentials (one scheme per usable flow), OpenID Connect
  - actions: a stable key from the operation id or method and path, a name, a group from the first tag, the method and path, parameters with their locations, the request body's content type, a single object input schema (each parameter by name, plus `body`), the first successful JSON response's schema, the auth schemes that work, and whether it's deprecated
- **Headers:** `Authorization`, `Content-Type` and `Accept` header parameters are left out of action inputs. The connection and the body type supply them.

## Consequences

- One code path serves all three formats, and an imperfect spec imports with warnings rather than not at all.
- The importer doesn't validate schemas against the JSON Schema meta-schema. A malformed schema shows up later, when an action's inputs are checked.
- OAuth2 implicit and password flows, HTTP digest, mutual TLS and external `$ref`s aren't supported. Each produces a warning naming what was skipped.
- Storage, overlays (renames, pagination, rate limits), re-import diffs and the `integration.action` step build on this definition in later changes.
