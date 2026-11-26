# The assistant

**Assistant** in the sidebar is a chat with your organization's [AI provider](ai.md), which can use Huxley's tools (design doc §10.3). Ask it to build a workflow, find out why a run failed, work a ticket, or answer questions about your organizations, clients, contacts, assets, tickets, integrations, workflows, runs, scripts, forms and datasets.

## How it works

- **It acts as you.** It uses the same tools as the [MCP server](mcp.md), called through Huxley's API with your own permissions. It can't see or do anything you couldn't, and what it does is audited with `via = mcp`.
- **Reading is automatic, changing is your call.** Tools that only read run straight away. Tools that create or change something (creating or updating a workflow draft, test-running a workflow, adding a ticket note, changing a ticket, or running one of your [workflow tools](mcp.md#your-workflows-as-tools)) wait, and you see exactly what it wants to do and approve or deny each one. A denied action is reported back to the model.
- **Drafts only.** It saves workflows as drafts: a person publishes them.
- **Your key, your budget.** It uses the organization's AI provider (Settings → AI) and counts toward its monthly budget. Each turn is in the AI call log as `assistant`.
- **Your conversations.** They're kept per person, and only you can see yours. Delete them any time.
- **Answers appear as they're written,** and each tool call shows while it runs. An answer finishes and is saved even if you leave the page.

- **From a ticket or a client:** **Ask the assistant** on a ticket or client page opens a conversation about it (ADR-053). It knows which ticket or client "this" is and reads it with its tools: it can summarize a ticket, find similar ones and say who should take it. With your approval, it can add an internal note or change the priority, queue or assignee.
- **Tools follow your modules:** service desk, client and asset tools are offered only while those modules are on (see [the MCP server](mcp.md#what-it-can-do)).

Using it needs `ai:use` at the organization (Technicians have it), plus an AI provider set up there or above.

## The API

| | |
|---|---|
| `GET /api/v1/assistant/conversations` | your conversations |
| `POST /api/v1/assistant/conversations` | `{ orgId, text }`: start one; returns it answered, or with actions waiting |
| `GET /api/v1/assistant/conversations/{id}` | messages, tool calls and results, and `pending` actions |
| `POST /api/v1/assistant/conversations/{id}/messages` | `{ text }`: send a message |
| `POST /api/v1/assistant/conversations/{id}/decisions` | `{ decisions: { "<call id>": true \| false } }`: approve or deny waiting actions |
| `DELETE /api/v1/assistant/conversations/{id}` | delete a conversation |

### Streaming

Send `Accept: text/event-stream` to the start, message and decision endpoints to get the answer as it's written. The response is a stream of Server-Sent Events, each with a JSON `data`:

| Event | Data |
|---|---|
| `text` | `{ text }`: the next piece of the assistant's text |
| `turn_ended` | One model turn ended. Text after this starts a new message. |
| `tool_started` | `{ id, name }`: a tool call started (only tools that run without approval) |
| `tool_finished` | `{ id, isError }` |
| `done` | `{ conversation }`: the conversation as saved, as `GET` returns it. The last event. |
| `failed` | `{ problem }`: why it couldn't answer (a problem object). The last event. |

Problems found before answering starts (validation, permissions, an action waiting for approval) are ordinary error responses, not events.
