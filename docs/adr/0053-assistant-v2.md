# ADR-053: Assistant v2: tool groups by module, and context from where it's opened

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8's Assistant v2 should be context-aware on tickets and client views, with service desk and clients/assets MCP tool groups. The assistant (ADR-020) already acts as the person through the MCP tools and asks before changing anything. Its tools were all about building workflows.

## Decision

**Tool groups, by module.** Besides the core tools, the MCP server and the assistant offer groups that appear only while their module is on: instance-wide for MCP, and at the conversation's organization for the assistant.

| Group | Module | Tools |
|---|---|---|
| Service desk | `servicedesk` | `find_tickets`, `get_ticket`, `get_ticket_notes`, `recommend_assignees`, `similar_tickets`, `add_ticket_note`*, `update_ticket`* |
| Clients | `crm` | `list_contacts`, `get_client_health` |
| Assets | `cmdb` | `find_assets`, `get_asset` |

\* Changes something, so the person approves each call.

Every tool is a call to Huxley's API with the person's own credentials, so permissions, scoping and audit are unchanged. `update_ticket` reads the ticket's current version first, so the change isn't refused as stale. It can set priority, queue and assignee, not status, and adding a note defaults to internal.

**Context.** A conversation can start with `context: { kind: ticket | client, id }`:

- The API checks the person can read it (`ticket:read` or `crm:read` there) and stores it with a label ("T-1042 “Printer jams”", or the client's name).
- Every turn's instructions say what the conversation is about, and which tools read it.

The ticket and client pages have **Ask the assistant**, which opens it with that context.

## Consequences

- Technicians can work a ticket conversationally: summarize it, find similar ones, see who should take it, and with approval, note or route it.
- Tools for disabled modules never reach the model, keeping its choices relevant and tokens down.
- Closing or resolving tickets stays a person's job in the ticket view for now. It's the next candidate tool, once there's a clear approval story for public replies.
