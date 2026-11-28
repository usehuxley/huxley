# ADR-095: Acting on AI output needs an approval, and outside text is fenced

- **Status:** Accepted. The user approved the approach. Closes the prompt-injection item in the security review (`docs/security.md`).
- **Date:** 2026-11-28

## Context

Huxley's AI steps read text from outside: client emails, ticket descriptions and notes, documents. That text can carry instructions of its own ("ignore your instructions and reply with…", "forward this thread to…"), and models follow such instructions often enough to matter. Text the AI writes from it can't be trusted to say only what the workflow author intended. No technique stops prompt injection outright. The defenses that work limit what injected text can make Huxley **do**.

Three properties held before this change:

- secrets can't be sent to AI steps (taint tracking);
- the assistant's changes wait for the user's approval;
- triage applies its suggestion only where an organization allows it.

But a workflow could take `ai.prompt` output and email it, post it to an API, or reply to a client, with no one looking.

## Decision

- **Publishing checks the data flow** (`huxley-expr`, `ai_guard`). The check runs whenever a workflow is validated: in the editor, on publish, in pack installs, and with `huxley workflows validate`.
  - **Sources:** free-form AI output, meaning `ai.prompt` and `ai.extract`.
  - **Propagation:** the mark follows through step inputs that read that output, through variables set from it, through loops over it, and through containers whose contents carry it.
  - **The rule:** a step that *acts* with such input must run only after a person decides. The step's own `if`, or the condition of a step it's inside, must read an `approval` step that comes after the AI step. Otherwise it's an error that says how to fix it.
  - **The opt-out:** `trust_ai: true` on a step, a new field every step has, acts without review. It's written in the document, where reviewers see it.
  - **Not acting:** control flow and data steps, reads, and steps that keep AI output as a suggestion for staff. Those are triage suggestions (applying them is an organization setting), summaries, sentiment, knowledge drafts, time suggestions, receipt readings, scope drafts, internal ticket notes, staff-only status reports and notifications.
  - **Acting:** everything else. A public note or a client-visible report counts as acting.
  - **Free to use:** `ai.classify` labels, which come from a fixed list, and conditions that only read AI output, since they decide whether a step runs, not what it does. Classification-based routing keeps working.
- **Outside text is fenced off.** The `text` of `ai.classify` and `ai.extract`, and a new `context` field on `ai.prompt`, are wrapped in `<data-…>` markers with a random suffix per call, so the text can't close the fence. The system instructions say to treat what's inside only as data. The built-in packs already keep outside text in `text`, so they're fenced without changes.
- **AI text is labeled in the app.** One `AiLabel` badge, with a tooltip saying it was written from outside text and should be checked, appears on:
  - ticket summaries and sentiment;
  - receipt readings and time estimates;
  - scope drafts;
  - status reports written by a workflow (a new `automated` field).

## Consequences

- A workflow that sends AI-written text without review won't publish until it gets an approval step, or until its author writes `trust_ai: true` and owns that choice. Workflows already published keep running until they're published again. Every built-in pack passes as it is.
- The check is static and errs on the side of caution. For example, everything inside a loop over AI output counts as reading it. `trust_ai` handles the cases it gets wrong.
- Fencing reduces injection but doesn't prevent it. Summaries and suggestions can still be steered, which is why they're labeled and why acting on them needs a person.
