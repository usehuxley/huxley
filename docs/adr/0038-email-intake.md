# ADR-038: Email intake: mailboxes, stored messages, and email to ticket as a step

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Design doc §8.4 has mail reaching the service desk through mailboxes (Microsoft 365 or IMAP). The implementation plan's principle 2 says this runs as **system workflows**, not hidden background code:

- **"Poll mailbox"** emits `email.received`.
- **"Email to ticket"** threads replies, matches the sender, filters automatic mail and loops, strips quoted text, and creates a ticket or adds a note.

Getting mail out of a provider and turning a message into a ticket are separate concerns. The second one doesn't depend on which provider the mail came from.

## Decision

### Mailboxes and stored messages

**A mailbox** (`sd_mailboxes`) belongs to an organization, usually the MSP. It has:

- an address, and a kind: `forward`, `imap` or `graph`
- the rules for its mail:
  - the ticket type and queue for new tickets
  - what to do with an unknown sender
  - an organization for tickets from unknown senders
  - whether to strip quoted replies

**Arrival.** A message arrives by being posted to its mailbox (`forward`), or by being fetched (`imap` and `graph`, which come in later PRs). When it arrives:

- The raw `.eml` is stored in object storage, so nothing is lost.
- What Huxley needs (ids for threading, addresses, subject, plain text, attachment list, why it looks automatic) goes into `sd_inbound_emails`, unique by mailbox and `Message-ID`. A message posted or fetched twice is stored once.
- It emits `email.received` in the mailbox's organization, with the email's id.

**Parsing** lives in `huxley-mail` (with `mail-parser`) as pure functions, unit tested:

- spotting automatic mail
- stripping replies and signatures
- the `[HX#1042]` subject token

### Email to ticket

**The `core.ticket.from_email` step** turns a stored email into ticket work. Its outcome is recorded on the email, and running it again does nothing new.

1. **Skip automatic mail:** `Auto-Submitted`, `Precedence: bulk/list/junk`, auto-reply headers, list headers, bounces.
2. **Skip loops:** a sender that is one of Huxley's mailboxes, or more than 20 messages from one sender to a mailbox in an hour.
3. **Thread:**
   - It first looks for the subject token and the ticket with that number.
   - Otherwise it uses `In-Reply-To` and `References`, matched against earlier emails' `Message-ID`s.
   - A reply becomes a public note from the requester contact.
   - A reply to a resolved ticket reopens it. A reply to a closed ticket opens a new one.
4. **Match the sender:**
   - It looks for a contact with the sender's address.
   - Otherwise, when the policy is `create_contact`, it creates one. The contact goes at the company where other contacts share the sender's domain (personal-mail domains don't count), or else at the mailbox's organization for unknown senders.
   - The `no_requester` policy opens the ticket there without a requester. `ignore` drops the message.
5. **Create the ticket** at the contact's company:
   - Its source is `email`.
   - It takes the mailbox's type and queue, and the subject without `Re:`/`Fwd:` prefixes.
   - Its text is stripped of quoted replies and signatures.
   - The SLA applies as for any ticket.
6. **Attachments** (not inline images) become the ticket's attachments. Tickets are now an entity type, so they also get custom fields and tags.

**Scope.** The step only works on tickets at the run's organization and those below it, like the other record steps. It fails with `module_disabled` where the service desk is off.

**The built-in pack** `huxley/servicedesk-email` ships the *Email to ticket* workflow. It's triggered by `email.received` and calls the step, so it can be viewed, copied and changed like any workflow.

### API

- **Mailboxes:** managed with `servicedesk:configure`.
- **Posting a raw message:** `POST /api/v1/mailboxes/{id}/messages`, with a body of `message/rfc822`. It needs `mailbox:ingest`, which suits a service account used by a mail server's forwarding rule.
- **A mailbox's recent emails and their outcomes:** readable with `servicedesk:configure`.

## Consequences

- **The core can be tested without a mail provider:** a message posted to a `forward` mailbox exercises everything after fetching.
- **What comes next:**
  - IMAP and Graph fetching (with polling, IDLE and subscriptions) plug in before `email.received`.
  - Outbound replies, with the token and `Message-ID`s recorded for threading, come with them.
