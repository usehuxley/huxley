# ADR-081: Expense reports go to the person's team leads through an approval workflow

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.16: expense reports go to the person's lead or approver through an approval workflow, with comments, change requests, policy flags and locking once approved.

## Decision

- **A report** bundles one person's entries: drafts, or entries sent back to them. All of them must be under one MSP, and the report sits at that MSP for row-level security. It has a title, its reviewers, totals per currency (what was spent, and what's to be paid back), and a count of each policy flag.
- **Submitting:**
  - The entries become `submitted`, and locked.
  - `expense_report.submitted` is raised. Its payload has the reference, the person, the totals, the flags, and the reviewers: the leads of the person's teams (`team_members.lead`, as for timesheets), or `role:admin` when there are none.
  - The built-in **Expense approval** pack runs an approval step (changes allowed, seven days to decide) and then `core.expense.report_decide`.
- **Deciding:**
  - Approved locks the entries for good: reimbursement, in E5, takes them from there.
  - Changes requested and rejected both send the entries back to the person, as `rejected` drafts they can change and submit in a new report. Rejected also closes the report.
  - Each decision raises its event and tells the person.
- **Comments** on a report, or on one of its entries, reach whoever submitted it and anyone @mentioned.

## Consequences

- Billing (E4) takes only approved billable entries, so approval gates what clients are charged.
- Reimbursement (E5) moves approved reports to `reimbursed`.
