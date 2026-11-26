# ADR-054: Changes extend change tickets; approval is a workflow

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 9's change management (design doc §8.6) covers:

- change types, a risk questionnaire, affected CIs from the dependency map
- approval policies with CAB and client approvers, including the portal
- a change calendar, freeze windows, conflict detection, and the post-implementation review

The exit criterion: a high-risk normal change routes to CAB, shows impacted CIs, respects a freeze window, and is approved by a client approver in the portal.

## Decision

**A change is a ticket of the `change` type**, with its change details in `sd_changes` (one row per ticket, created on first read). Changes keep everything tickets already have: notes, SLAs, assignment, the portal, links to incidents and problems. The details are:

- **Type:** standard (pre-approved), normal or emergency.
- **Risk:** five questions with weighted options: who would notice, downtime, testing, backout, familiarity. The answers score 0–13: low ≤ 3, medium ≤ 7, high above.
- **Plans:** implementation, backout and test.
- **A window, and affected assets.**

**Lifecycle:** draft → submitted → approved or rejected (a rejected change can be edited and resubmitted) → implemented (successful, failed or backed out) → reviewed (the PIR).

**Submitting checks the change:** every risk question answered, a window, and plans (unless it's standard).

- **Freezes:** outside emergencies, a window in a freeze is refused. Freezes are `sd_change_freezes`, set at an organization for it and below.
- **Conflicts:** other submitted or approved changes overlapping in time on the same assets are returned as conflicts, a warning rather than a block.
- **Standard changes** are approved on submission.

**Impact** is the dependency map's: each affected asset's dependents, up to five steps, through `cmdb::relationships::impact`, among the assets the reader can see.

**Approval is a workflow.** `change.submitted` carries the type, risk level and score, the window, the affected-asset count and the conflict count. The built-in **Change approvals** pack:

1. asks the CAB with an `approval` step. The default approvers are the Administrator role at the change's company; organizations point it at their CAB team.
2. for high-risk normal changes, then asks the company's client approvers in the portal (the existing `client_approvers`).
3. records the outcome with **`core.change.decide`**, raising `change.approved` or `change.rejected`.

Event runs run at the change's company, so the portal approvers are the right client's. Policies are edited as workflows: by risk, type, client, or anything in the event.

**The change calendar** (`/changes`) shows planned changes and freezes for four weeks.

## Consequences

- No separate change entity to keep in step with tickets. Change-specific data lives beside the ticket, and the ticket type gates it.
- Without the pack, a submitted normal change waits: approval policy is deliberately the organization's workflow.
- The questionnaire is fixed for now. Making it configurable per organization, with the same scoring, is a later step.
