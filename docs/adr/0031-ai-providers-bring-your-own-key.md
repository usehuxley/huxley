# ADR-031: AI providers are per organization, bring your own key

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-24

## Context

AI features (workflow steps, and later the assistant) call model providers (design doc §10.1). MSPs bring their own keys and decide which provider serves which client. Spend must be bounded and visible, and keys must stay secret.

## Decision

- **Providers:** `huxley-ai` calls four kinds over HTTP with one request shape (system, prompt, optional JSON Schema):
  - **Anthropic** (Messages API, `anthropic-version: 2026-10-21`, default model `claude-opus-5-5`)
  - **OpenAI** (Chat Completions)
  - **Azure OpenAI** (a deployment on the org's resource)
  - **OpenAI-compatible** servers

  Structured output uses each provider's JSON-schema mode (Anthropic's `output_config.format`, OpenAI's `response_format`). Refusals, truncation, and retryable failures (429, 5xx, Anthropic's 529) are distinct errors.
- **Refusal fallback:** Anthropic requests enable server-side refusal fallback (`fallbacks: "default"`), so a declined request is retried on a fallback model within the same call.
- **One provider per organization**, used by it and by every organization below that has none of its own. The key is encrypted with the organization's data key, bound to the provider row, and never returned.
- **Budget:** an optional monthly token budget per provider, counted across every organization using it. A call once it's used up fails without being sent.
- **Log:** every call is logged, with organization, feature, run, model, tokens and outcome (`ok`, `refused`, `failed`, `over_budget`).
- **Egress:** calls go through the engine's egress client, so a self-hosted, OpenAI-compatible base URL is subject to the same egress policy as HTTP steps.
- **Permission:** setting the provider and reading usage need `ai:manage`.

## Consequences

- Each MSP controls cost per provider, and a client can get its own provider (and bill) by setting one.
- Embeddings, streaming and per-feature model choice are later additions; workflow steps may already pick a model per call.
