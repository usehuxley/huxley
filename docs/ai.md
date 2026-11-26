# AI providers

Huxley's AI features use **your own AI provider key** (design doc §10.1, ADR-031). It supports Anthropic, OpenAI, Azure OpenAI, and OpenAI-compatible servers, plus Voyage AI for embeddings.

## Setting up a provider

Set it under **Settings → AI**, or with `PUT /api/v1/ai/provider`. Both need `ai:manage`.

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

## AI steps in workflows

```yaml
- id: gist
  type: ai.prompt
  with: { prompt: "{{ trigger.body.text }}", system: "Summarize in one line." }
- id: kind
  type: ai.classify
  with: { text: "{{ trigger.body.text }}", labels: [billing, onboarding, outage, other] }
- id: who
  type: ai.extract
  with:
    text: "{{ trigger.body.text }}"
    schema: { type: object, properties: { name: { type: string }, start: { type: string, format: date } } }
```

| Step | Output |
|---|---|
| `ai.prompt` | `{text, model}` |
| `ai.classify` | `{label, model}`: always one of `labels`, using the provider's structured output |
| `ai.extract` | the data, as `schema` describes it (made strict: every field present, with `null` for anything the text doesn't say) |

How the steps behave:

- **Provider:** they use the run's organization's provider, its own or the nearest above, and `model` picks another model for one step.
- **Logging:** each call is logged against its run.
- **Failures:** a refusal, an exhausted budget or a provider error fails the step with `kind: ai`. Provider errors also carry `retryable` and `status`, for retry policies.
- **No secrets:** AI steps refuse input that contains secrets, or values derived from them, so secrets are never sent to a provider.

Built-in packs that use these steps:

- **AI sentiment** judges the tone of client messages for [client health](health.md#sentiment).
- **AI triage** suggests a queue, priority and needed skills for new tickets, with an accuracy evaluation on labeled samples ([AI triage](servicedesk.md#ai-triage)).
- **AI ticket insights** keeps a summary, entities and a next step on each ticket ([Ticket insights](servicedesk.md#ticket-insights)).

## Embeddings provider

Searching [tickets by meaning](servicedesk.md#search-by-meaning) turns text into *embeddings*, lists of numbers that are close together for texts about the same thing. Anthropic has no embeddings API, so embeddings have their own provider, often another vendor than the one that answers.

Set it in the **Embeddings provider** card under **Settings → AI**, or with the same endpoints and `"use": "embeddings"` (in the body for `PUT`, as `?use=embeddings` otherwise):

```json
{ "orgId": "…", "use": "embeddings", "provider": {
    "kind": "openai",
    "apiKey": "sk-…"
} }
```

- **Kinds:** `openai` (default model `text-embedding-3-small`), `voyage` (Voyage AI, default `voyage-3.5`), `azure_openai` and `openai_compatible` (name the model). Not `anthropic`.
- **The same rules:** it's inherited down the tree, has its own key and monthly budget, and its calls are in the log.
- **Test:** embeds one word and shows how many dimensions the model gives.
- **Changing the model** re-embeds every ticket it serves, since vectors from different models can't be compared.

## Budgets and the log

Every AI call is logged with its organization, feature, run, model, tokens and outcome: `GET /api/v1/ai/calls?orgId=…`.

When a provider's monthly budget is used up, further calls fail without being sent, until the month ends or you raise the budget. `GET /api/v1/ai/provider` shows this month's usage.

## Good to know

- **Refusal fallback:** Anthropic requests have server-side refusal fallback on, so a request the model declines is retried on a fallback model within the same call. A request that's still declined is an error the caller sees.
- **Egress policy:** AI calls go through Huxley's egress policy, like HTTP steps, so an OpenAI-compatible server on a private address must be allowed there.
