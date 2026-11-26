# ADR-048: Dispatch recommendations are a transparent score, not a model's guess

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8 asks for dispatch recommendations "with reasons", weighing:

- skills and certifications
- availability and load
- history and familiarity with the client

The skills dispatch strategy already ranks queue members by skill match and skips the unqualified and unavailable. But it neither explains a choice nor weighs history or familiarity. A recommendation people act on has to be predictable and explainable. It should also run without an AI provider or per-ticket AI cost.

## Decision

**Requirements can be set on a ticket** (`wf_requirements.target_type = 'ticket'`) as well as its queue and type, by anyone with `ticket:write` on it. `fit` merges the three; a skill asked for twice counts at the higher level.

**A score out of 100, from fixed parts:**

| Part | Points |
|---|---|
| Skill match (how well levels meet the requirements) | 40 |
| Available now (not away, within working hours) | 20 |
| Load: 15 × (lightest + 1) / (theirs + 1) open tickets | 15 |
| History: tickets resolved in this queue, 90 days (10 for full points) | 15 |
| Familiarity: the client's tickets worked, 180 days (5 for full points) | 10 |

- **Qualification:** someone missing a required certification scores at most 30 and ranks after everyone qualified.
- **Reasons and cautions:** every part that counts becomes a reason ("Resolved 6 in Network lately", "Worked 3 of Acme's tickets", "2 open tickets (the lightest load)"). Gaps become cautions ("Missing: CCNA", "Away", "Assigned now").
- **Without the workforce module**, skills and availability aren't weighed: both count in full and give no reasons.

**Candidates** are the queue's members (people and teams). If the queue has none, they're the people whose role allows working tickets at the ticket's organization or above.

**Where it shows:**

- the ticket page's *Who should take it* (top three, one-click Assign)
- `GET /api/v1/tickets/{id}/recommendations`
- the `core.dispatch.recommend` step, so workflows can assign or notify with the reasons

## Consequences

- The same inputs always give the same ranking, and the reasons say exactly why.
- The weights are fixed for now. They can become an organization model like client health's (ADR-044) if MSPs ask.
- AI triage can set a ticket's needed skills (a follow-up), and these recommendations then weigh them without any change here.
