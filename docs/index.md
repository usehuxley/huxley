# Huxley

Huxley is an automation platform for managed service providers (MSPs). It runs workflows against the systems you and your clients use, across your whole organization tree: your MSP, and every client below it.

## The pieces

- **Organizations.** Your MSP and its clients form a tree. Most things belong to an organization and are shared with the organizations below it: integrations, scripts, datasets, AI providers. People see only the organizations their roles cover.
- **[Workflows](workflows.md).** Documents in the Huxley Workflow Format (HWF): what starts them, the steps they run, and what they return. Build them in the visual editor, as YAML, or [from code](workflows.md#workflows-as-code). Huxley runs them durably and records every step.
- **[Integrations](integrations.md).** APIs imported from their OpenAPI specs, with connections holding their credentials. Microsoft Graph and Slack are built in.
- **[Forms](forms.md).** Forms that start workflows, for your staff or through public links.
- **[Scripts](runners.md).** PowerShell, Python and JavaScript, run in sandboxes on your runners.
- **[Datasets](datasets.md).** Typed tables of data kept in Huxley, such as every client's Microsoft 365 users, synced from integrations.
- **[AI](ai.md).** Your own AI provider and key: AI steps in workflows, the [assistant](assistant.md), and an [MCP server](mcp.md) for AI clients like Claude Code.
- **[Packs](packs.md).** Signed bundles of all of the above, to share between instances.

## Where to start

[Build your first workflow](getting-started.md): from an empty organization to a published workflow that a form starts, in about ten minutes.

The HWF JSON Schema is at [`/schemas/hwf/v1.json`](/schemas/hwf/v1.json). Point your editor at it to get autocomplete for workflow files.
