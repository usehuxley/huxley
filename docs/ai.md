# AI providers

Huxley's AI features use **your own AI provider key** (design doc §10.1, ADR-031). It supports Anthropic, OpenAI, Azure OpenAI, and OpenAI-compatible servers.

## Setting up a provider

Set it per organization with `PUT /api/v1/ai/provider`, which needs `ai:manage`:

```json
{ "orgId": "…", "provider": {
    "kind": "anthropic",
    "apiKey": "sk-ant-…",
    "monthlyTokenBudget": 5000000
} }
```

The provider fields:

| Field | |
|---|---|
| `kind` | `anthropic`, `openai`, `azure_openai` or `openai_compatible` |
| `model` | The model (default for Anthropic: `claude-opus-5-5`). For Azure, the deployment's model. |
| `baseUrl` | Required for Azure (`https://<resource>.openai.azure.com`) and OpenAI-compatible servers. |
| `deployment` | Azure: the deployment's name (default: the model). |
| `apiKey` | Required when first set; leave it out to keep the current key. It's stored encrypted and never shown again. |
| `monthlyTokenBudget` | Optional: tokens (in plus out) the provider may use a calendar month. |

How settings apply:

- **Inheritance:** a provider set at your MSP serves it and every client below that has no provider of its own. Set one on a client to give it its own key and bill.
- **Test:** `POST /api/v1/ai/provider/test?orgId=…` sends a tiny request and shows the reply or the error.
- **Removing:** `DELETE /api/v1/ai/provider?orgId=…` removes an org's own provider.

## Budgets and the log

Every AI call is logged with its organization, feature, run, model, tokens and outcome: `GET /api/v1/ai/calls?orgId=…`.

When a provider's monthly budget is used up, further calls fail without being sent, until the month ends or you raise the budget. `GET /api/v1/ai/provider` shows this month's usage.

## Good to know

- **Refusal fallback:** Anthropic requests have server-side refusal fallback on, so a request the model declines is retried on a fallback model within the same call. A request that's still declined is an error the caller sees.
- **Egress policy:** AI calls go through Huxley's egress policy, like HTTP steps, so an OpenAI-compatible server on a private address must be allowed there.
