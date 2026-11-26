# ADR-046: Sentiment is judged by a workflow and recorded by a step

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8 asks for sentiment analysis as a health factor, and its exit criterion is "a declining-sentiment client is flagged with explanations". Every AI feature should be a visible system workflow, not hidden code. Judgments also have to be explainable and cheap enough to make on every client message.

## Decision

**Judging is a workflow.** The built-in **AI sentiment** pack's *Judge the tone of client messages* workflow runs on `ticket.created` (raised by a client: no staff actor) and on public client replies (email or portal). It runs `ai.extract` with a fixed rubric and a strict schema:

- `score`: an enum, -2 to 2
- `reason`: at most 15 words

Then it calls `core.sentiment.record`. Administrators can see, change or replace the prompt, the rubric and the triggers like any workflow, and every call is in the AI log against its run.

**Recording is a step.** `core.sentiment.record` stores one row per message in `cx_sentiment`: the note, or the ticket's description, with its company, contact, score and reason. Judging the same message again replaces its row.

**It's a health factor.** "Client sentiment" (default weight 2) is the 30-day average mapped to 0–100. It needs 3 judged messages.

**Falling sentiment flags a client.** The daily health computation compares the 30-day average with the 60 days before. With at least 3 messages in each window and a fall of 0.75 or more, it emits `client.sentiment_declining` with:

- `recent` and `previous` averages
- `explanations`: the three most negative recent messages, as "T-123: reason"

A snapshot records the flag, so a client is flagged at most once a week. The pack's second workflow tells administrators, with the explanations.

## Consequences

- Without an AI provider, or with the pack uninstalled, nothing is judged. The factor reads "too few to tell" and doesn't count.
- Explanations are the model's one-line reasons plus ticket references, not quotes, so notifications don't repeat what clients wrote.
- The score scale is coarse (5 levels) on purpose: it's stable across models, and averages over many messages carry the signal.
