# ADR-030: MCP tool calls go through the API, as the user

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-24

## Context

The MCP server (design doc §10.2) lets AI clients, such as Claude Code with the user's own key, list integrations and build, test and save workflow drafts. Every tool call must run as the authenticated user, pass the same permission checks as the API, and be audited as coming through MCP. A second implementation of each operation would drift from the API's rules.

## Decision

- **The transport is Streamable HTTP at `/mcp`.** JSON-RPC 2.0 messages arrive by POST and each gets a single JSON response; notifications get `202`. `GET` returns `405`: the server opens no streams. It supports `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list` and `resources/read`.
- **Auth is an API token** (`Authorization: Bearer`), so a token's scopes narrow what an MCP client may do. A request without a token gets `401` with `WWW-Authenticate`. OAuth 2.1 can be added later.
- **Each tool is a call to the API router itself**, made in-process with the client's own `Authorization` header. Permission checks, validation, organization scoping and errors are exactly the API's. A failed call comes back as a tool result with `isError` and the problem's detail, which the model can act on.
- **Audit:** in-process calls carry a request extension, which clients can't set, so their audit records say `via = mcp`.
- **Drafts only:** no tool publishes or deletes. AI-made workflows land as drafts that a person reviews and publishes. `test_workflow_document` test-runs a document without saving it, with mocked steps.
- **stdio:** `huxley mcp` relays newline-delimited JSON-RPC between stdin/stdout and a server's `/mcp`, for clients that launch servers as local processes.

## Consequences

- A new API endpoint becomes a tool with a few lines, and tools can't bypass the API's rules.
- In-process calls cost a router dispatch each, which is negligible next to the work they do.
- Published workflows exposed as tools (`mcp_tool`) and OAuth sign-in for MCP clients come later.
