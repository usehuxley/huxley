# API Conventions

Every Huxley HTTP endpoint follows these rules. They keep the API predictable for the web apps, the generated TypeScript client, MCP tools, and third-party integrators.

## Basics

- Base path: `/api/v1`. Probes (`/healthz`, `/readyz`) live at the root and are unversioned.
- JSON request and response bodies, UTF-8, `camelCase` field names.
- Identifiers are UUIDv7 strings. Timestamps are RFC 3339 in UTC.
- Every response carries `x-request-id`. Clients may send their own; otherwise the server generates one. Logs and traces include it.
- Requests may carry W3C `traceparent`. The server continues that trace.
- Body limit: 2 MiB unless an endpoint documents otherwise. Request timeout: 30 seconds.
- The OpenAPI document is served at `/api/v1/openapi.json` and exported with `huxley openapi`. Every endpoint must appear in it.

## Errors: RFC 9457 problem details

All errors use `Content-Type: application/problem+json`:

```json
{
  "type": "https://usehuxley.io/problems/validation",
  "title": "Validation failed",
  "status": 422,
  "detail": "Two fields are invalid.",
  "errors": [{ "pointer": "/name", "message": "Must not be empty." }]
}
```

- `type` is stable per error kind; clients branch on it, not on `title` or `detail`.
- `detail` is human-readable and specific to this occurrence. It never contains secrets, stack traces, or internal identifiers.
- `errors` holds field-level problems, each with a JSON Pointer into the request body.

| Status | `type` slug | When |
|---|---|---|
| 400 | `bad-request` | Malformed request, invalid cursor or header |
| 401 | `unauthorized` | Missing or invalid authentication |
| 403 | `forbidden` | Authenticated but not permitted |
| 404 | `not-found` | Resource doesn't exist, **or** the caller may not know it exists |
| 409 | `conflict` | Version conflict, duplicate, or invalid state transition |
| 422 | `validation` | Well-formed but invalid input |
| 429 | `rate-limited` | Too many requests; see `Retry-After` |
| 500 | `internal` | Unexpected error (details are logged, never returned) |
| 503 | `unavailable` | A dependency is down |

Return 404 instead of 403 when revealing a resource's existence would leak information across organizations.

## Pagination: cursors

List endpoints use opaque cursors, never offsets:

```
GET /api/v1/tickets?limit=50&cursor=eyJpZCI6Ii4uLiJ9
```

```json
{ "items": [ ... ], "nextCursor": "eyJpZCI6Ii4uLiJ9" }
```

- `limit` defaults to 50, maximum 200.
- `nextCursor` is `null` on the last page.
- Cursors are opaque. Clients must not build or parse them. They may expire; an invalid cursor returns 400.
- Results are ordered by a stable key (by default `id`, which is time-sortable).

## Filtering and sorting

```
GET /api/v1/tickets?filter[status]=open,pending&filter[priority][gte]=2&sort=-createdAt,id
```

- `filter[field]=a,b` matches any of the listed values.
- `filter[field][op]=value` with `op` in `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `contains`, `startsWith`, `isNull`.
- `sort` is a comma-separated field list; a leading `-` means descending.
- Each endpoint documents which fields are filterable and sortable. Unknown fields return 400.
- Authorization filters are always applied on top of user filters.

## Idempotency

`POST`, `PATCH`, and `DELETE` accept an `Idempotency-Key` header (1–255 visible ASCII characters, typically a UUID). Repeating a request with the same key and body within 24 hours returns the original response without applying the change again. Reusing a key with a different body returns 409.

Phase 0 validates the header. Response storage arrives with the first mutating endpoints in Phase 1.

## Concurrency

Mutable resources carry a `version` integer. Updates send it back (`If-Match: "<version>"` or a `version` field); a stale version returns 409 so clients never silently overwrite someone else's change.

## Partial updates

`PATCH` uses JSON Merge Patch (RFC 7396) semantics: omitted fields are unchanged, `null` clears a field.

## Field-level security

Fields the caller may not read (for example cost prices, internal notes, credentials) are omitted from responses entirely, not returned as `null`. Write-only fields such as secrets are accepted on input and never returned.
