# The assistant

**Assistant** in the sidebar is a chat with your organization's [AI provider](ai.md), which can use Huxley's tools (design doc §10.3). Ask it to build a workflow, find out why a run failed, or answer questions about your organizations, integrations, workflows, runs, scripts, forms and datasets.

## How it works

- **It acts as you.** It uses the same tools as the [MCP server](mcp.md), called through Huxley's API with your own permissions. It can't see or do anything you couldn't, and what it does is audited with `via = mcp`.
- **Reading is automatic, changing is your call.** Tools that only read run straight away. Tools that create or change something (creating or updating a workflow draft, test-running a workflow) wait, and you see exactly what it wants to do and approve or deny each one. A denied action is reported back to the model.
- **Drafts only.** It saves workflows as drafts: a person publishes them.
- **Your key, your budget.** It uses the organization's AI provider (Settings → AI) and counts toward its monthly budget. Each turn is in the AI call log as `assistant`.
- **Your conversations.** They're kept per person, and only you can see yours. Delete them any time.

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
