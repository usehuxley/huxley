# ADR-076: Invoices are reviewed on the quote review model, and an approved review approves the invoice

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.11 requires that no invoice goes to a client, the accounting system or any other integration until it's approved. It also says invoice approval uses the same review model as quotes (ADR-066):
- customizable rules;
- the approval as an editable workflow;
- review threads with line comments and `@mentions`;
- approve, request changes or reject, with a diff;
- an audited override.

Billing runs can be approved in one pass, with exceptions flagged. Phase 11's exit criteria add that an invoice changed by hand needs approval before it's sent or synced.

## Decision

- **Rules** are instance-wide, like quote review rules. The kinds are:
  - every invoice;
  - a line changed by hand;
  - a total above an amount;
  - a total that moved more than a percent from the client's last approved or sent invoice in the same currency;
  - the client's first invoice;
  - credit memos;
  - an expression over the invoice.

  One rule ships on, "Changed by hand", reviewed by administrators. A draft's reasons are refreshed whenever it's made or changed.
- **Revision.** Invoices gain a `revision`, bumped when lines or notes change; holds don't bump it. A review is of a revision. A change after the review means it needs a new one.
- **Submitting** keeps a snapshot and raises `invoice.submitted_for_review` for the built-in **Invoice review** pack: an approval step that allows "changes requested", then `core.invoice.review`.
- **Deciding.**
  - An approved review approves the invoice: it sets the issue and due dates and raises `invoice.approved`, unless the invoice was put on hold meanwhile.
  - Changes requested or rejected leave it a draft.
  - Whoever submitted it is told.
  - Unlike quotes, invoices have no owner, so the submitter is the person to tell.
- **Approving directly** works only when no rule matches, or a review of this revision was approved. `billing:review:override` approves anyway, recorded in the audit log and shown on the invoice.
- **Runs** are approved in one pass:
  - Drafts that need no review are approved.
  - Held drafts, and those needing review, come back as exceptions with the reason.
  - Optionally, those needing review are submitted.
- **Comments** follow the quote model: threads on the invoice or a line, with `@mentions` that notify (`invoice.mentioned`), resolvable.

## Consequences

- The accounting sync (B3) only takes approved invoices, so a review gates the sync with no further checks.
- Quote and invoice review share a shape but not code. If a third kind of review appears, extract the common part.
