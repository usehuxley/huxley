# ADR-047: AI triage is measured by running its own workflow on labeled samples

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8's exit criterion: AI triage accuracy measured on a labeled sample set, against a target set during the phase. Triage must stay a visible, editable workflow. The prompt, the model and the steps can then change, and that makes an evaluation worthless unless it tests the workflow as it is now.

## Decision

**Triage is a workflow.** The built-in **AI triage** pack's *Triage new tickets with AI* has two steps around an `ai.extract` call:

- **`core.triage.options`** gives the queues at the ticket's organization or above, with their descriptions as a prompt guide, plus the priorities. The extraction schema's `queue` is an enum of those names plus "(none)", so the model can only answer with a real queue.
- **`core.triage.apply`** keeps the suggestion (`sd_triage`) and applies it while no person has changed the ticket. It sets the queue only if the ticket has none, and the priority.

**Live agreement.** For resolved tickets with a suggestion in the last 90 days, it counts how often the suggestion's queue and priority match where the ticket ended up.

**Evaluations run the workflow itself.** The labeled sample set (`sd_triage_samples`) holds text with the right queue, priority or both. Samples are added by hand or taken from recently resolved tickets, which people confirmed by working them. An evaluation:

1. copies each sample's expected labels (`sd_triage_results`)
2. starts one manual run of the published workflow per sample, with `text`, `evaluation_id` and `sample_id`
3. records the answer through `core.triage.apply` against the evaluation instead of a ticket

The score is calculated from these rows:

- **Queue accuracy:** correct out of the samples with an expected queue. Failed runs count as wrong.
- **Priority accuracy**, counted the same way.
- **Misses**, each with the model's reason, or a link to the run that failed.

**The target** is the setting `triage.accuracy_target`: **80% of samples routed to the right queue** by default, set per organization. An evaluation passes once nothing is pending and its queue accuracy is at or above the target in force when it started.

## Addendum: suggest mode and needed skills

- **Suggest mode:** `triage.auto_apply` (on by default) lets an organization take only suggestions: they're recorded, never applied.
- **Needed skills:** with the workforce module on, `core.triage.options` also lists the skill catalog (`skill_names`, and in the guide), and the extraction asks for the skills a ticket needs. `core.triage.apply` keeps them with the suggestion. When it applies, it sets them as the ticket's own requirements at working level, unless the ticket has requirements already. Dispatch recommendations (ADR-048) then weigh them.

## Consequences

- Changing the prompt, model or steps and re-running the evaluation measures exactly what will run on live tickets.
- Evaluations cost one AI call per sample, so the sample set is capped at 500.
- Triage never overrides a person: once anyone has changed a ticket, suggestions are only recorded.
