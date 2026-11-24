# The MCP server

Huxley is an [MCP](https://modelcontextprotocol.io) server, so AI clients such as Claude Code, Claude Desktop or IDE assistants can explore your integrations, and build, test and save workflow drafts, using **your own AI key** (design doc §10.2, ADR-030).

## Connecting

Add the server's `/mcp` URL to your client. The first time it connects, your browser opens Huxley: sign in if you aren't, check the app's name, and select **Connect**.

```sh
claude mcp add --transport http huxley https://huxley.example.com/mcp
```

What happens behind that:

- The client discovers how to sign in from `/mcp` itself, registers, and uses OAuth 2.1 with PKCE (ADR-032).
- **Access:** the app gets **everything your roles allow**, never more. A client can ask for less by requesting permission keys as scopes, such as `workflow:read workflow:write`.
- **Tokens:** the app's token works only with the MCP server, not the rest of the API. It lasts an hour and the client renews it by itself.
- **Disconnecting:** connected apps are listed under **Profile → Connected apps**, and you get a security notice when one connects. **Disconnect** stops one at once.

The server needs `auth.public_url` (its address as browsers see it) for sign-in to work.

### With an API token instead

For scripts, CI, or clients without OAuth, make an API token (**Profile → API tokens**) scoped to what the client may do. For building workflows, that's `workflow:read`, `workflow:write`, `integration:read`, `org:read` and, to read run details, the default read scopes.

Over HTTP:

```sh
claude mcp add --transport http huxley https://huxley.example.com/mcp \
  --header "Authorization: Bearer hxp_…"
```

Over stdio, for clients that start servers as local commands:

```json
{ "mcpServers": { "huxley": {
    "command": "huxley", "args": ["mcp"],
    "env": { "HUXLEY_SERVER": "https://huxley.example.com", "HUXLEY_TOKEN": "hxp_…" } } } }
```

## What it can do

| Tool | |
|---|---|
| `list_organizations`, `list_integrations`, `get_integration`, `list_actions` | what there is to build with |
| `get_workflow_schema` (also the resource `huxley://schemas/hwf/v1.json`) | every step type and field |
| `list_workflows`, `get_workflow` | existing workflows and their drafts |
| `validate_workflow` | a document's problems, without saving it |
| `test_workflow_document` | test-run a document without saving it, with mocked steps; returns a run id |
| `create_workflow_draft`, `update_workflow_draft` | save it as a draft |
| `test_run_workflow` | test-run a saved draft for real |
| `get_run`, `list_runs` | runs, with each step's inputs, outputs and errors |
| `list_scripts`, `list_forms`, `list_datasets`, `query_dataset` | scripts, forms and data |

Documents can be YAML text or JSON objects.

## Your workflows as tools

A published workflow with an `mcp_tool` trigger becomes a tool of its own:

```yaml
spec:
  triggers:
    - { type: mcp_tool, description: "Resets a user's password and emails them a temporary one." }
  inputs: { type: object, required: [email], properties: { email: { type: string } } }
```

How workflow tools work:

- **Naming:** each is `run_<workflow key>` (hyphens become underscores; a suffix tells apart same-key workflows of different organizations).
- **Inputs and description:** the workflow's input schema becomes the tool's, and the trigger's `description` tells the model what it does (default: the workflow's description).
- **Calling one** starts a run of the **published** version as you, and waits up to 25 seconds. It returns the outputs, or the run's id if it's still going.
- **Who sees them:** only people who may run the workflow (`workflow:run`) see it as a tool.
- **In the [assistant](assistant.md):** these tools are offered there too, and every run waits for your approval.

## Safety

- **Every tool call runs as you.** It's made through Huxley's own API with your token, so it gets your permissions, narrowed by the token's (or connected app's) scopes, and the same validation. A refused call comes back as an error the model can read.
- **Drafts only.** No tool publishes or deletes anything. A person reviews and publishes what an AI built.
- **Audited.** Everything done through MCP is in the audit log with `via = mcp`.
