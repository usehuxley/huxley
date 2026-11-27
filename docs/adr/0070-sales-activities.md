# ADR-070: Sales activities live with their records, and sales email is logged from a mailbox by a workflow

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10 asks for activities and communications: tasks, calls, meetings, and email logging, through mailbox sync and a BCC address. Huxley already receives email through mailboxes (ADR-038) for tickets.

## Decision

- **One table of activities**, each one a task, call, meeting, note or email, on a lead, opportunity, quote or contact.
  - An activity's organization is its record's: the provider's for a lead, the client's for the others.
  - Access is `sales:read` or `sales:write` there. Contacts, opportunities and quotes on one activity must belong to the same client.
  - Tasks have an owner and a due time, and are done or not. Giving one to someone else notifies them (`sales.task`).
  - Calls are logged as done. Notes are done when written.
- **Mailboxes gain a purpose:** tickets (the default) or `sales_log`.
  - The ticket intake passes a sales mailbox's messages by, recorded as `ignored_sales_log`.
  - `email.received` now carries the purpose, so a workflow can filter on it.
- **Logging is a workflow.** The built-in **Sales email logging** pack triggers on `email.received` filtered to sales mailboxes. It runs a new `core.sales.log_email` step, which:
  - matches the message's from, to and cc addresses (other than the mailbox's own) to contacts and to open leads;
  - logs one email activity per match, with a direction: in from them, out to them;
  - puts a contact's activity on their client's most recently changed open opportunity;
  - makes logging idempotent (a unique index on the email and contact or lead).

  A BCC address is a forwarded mailbox. "Mailbox sync" is an IMAP or Graph mailbox with the sales purpose.

## Consequences

- Organizations can change how email is matched or filed by copying the pack's workflow, for example to log only to opportunities, or to skip internal domains.
- Calendar sync of meetings, and per-user mailbox sync through Graph with per-user consent, can come later on the same table.
