# ADR-066: Quotes are reviewed internally by rules, a workflow and a third approval outcome

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 10 calls for internal review of quotes before they go out: thin margins, big deals, deep discounts and new clients get a second pair of eyes, reviewers can send a quote back rather than only reject it, and people discuss lines in place.

## Decision

- **Review rules** are instance-wide (kept with `sales:configure` beside the stages): every quote; a first-year margin below a percent; a first year above an amount; a line discounted more than a percent; the client's first quote; a product category; or an expression over the quote (`{{ }}` optional). Each names its reviewers as approval-step approvers (`role:`, `team:`, `user:`). A quote keeps the rules it matches, worked out again on every save.
- **Review runs as a workflow.** Submitting a draft keeps a snapshot of it and raises `quote.submitted_for_review` with the reasons, the reviewers, the totals and what changed since the last review. The built-in **Quote review** pack asks the reviewers with an `approval` step and records their decision with a new `core.quote.review` step, which raises `quote.approved`, `quote.changes_requested` or `quote.rejected` and tells the quote's owner. Organisations can add levels (a second approval above a total) or swap reviewers in the workflow, without code.
- **A third approval outcome.** The `approval` step gains `changes: true`, letting approvers answer `changes_requested` as well as approved or rejected (staff in **Approvals** or on the quote, contacts in the portal). HWF v1 only grows: the field is optional and defaults to off.
- **The send gate.** A quote that matches a rule can be marked sent only with an approved review of its current revision. Someone with `sales:review:override` (administrators) can send it anyway; the quote shows it, and the override is audited. Any change after review needs a new one; a decision on a stale review is refused.
- **What changed** compares the quote with the snapshot of its last decided review, line by line, using line ids that now survive saves.
- **Comments** sit on the quote or a line (kept by name if the line goes), with replies, `@mentions` that notify (`quote.mentioned`), and resolve.

## Consequences

- Review policy stays in workflows, where admins already keep approvals, and the quote only holds the outcome.
- Without the pack installed, quotes that need review stay in review until someone with the override sends them; the settings page says so.
- The third outcome is available to any workflow's approvals, not just quotes.
