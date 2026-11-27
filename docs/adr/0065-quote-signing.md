# ADR-065: Clients answer quotes in the portal with a typed name and a drawn signature

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

The Phase 10 exit criterion has a client e-accept a quote in the portal. An acceptance has to hold up later: who agreed, to exactly what, and when.

## Decision

- **Who:** the quote's contact can see and answer it; the company's client approvers and admins see and answer all its quotes (portal permission `portal.quote:read`). Drafts are never shown.
- **What they see:** the quote as sent, without costs or margin. Options are offered as tick boxes, and the totals follow what's ticked.
- **Accepting** takes the person's full name, a signature drawn on a pad (kept as a PNG, at most 200 KB), the options taken, and a tick that they accept the quote and its terms for their organization. **Declining** takes a name and, optionally, why.
- **The record:** each answer stores the revision answered and the SHA-256 of that revision's sent snapshot (what was agreed to), the contact and portal account, their address and browser, and the time. One answer per quote. The answer then decides the quote exactly as a staff-recorded one does, so accepting wins the opportunity.
- **Guards:** a quote that changed since it was opened (a newer revision) or is past its *valid until* date can't be accepted; an answered quote can't be answered again.
- **Staff** see the record on the quote, with the drawn signature.
- **Options** count only once the client takes them, everywhere (the builder, the totals, the PDF).

## Consequences

- No third-party e-signature service is needed; the record is in Huxley's audit trail and the quote's revisions.
- A public link for contacts without a portal account can reuse the same answer function later.
