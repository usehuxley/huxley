# ADR-064: A quote is one editable document; each send keeps a revision

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

CPQ needs quotes built from the catalog, with one-time and recurring lines, options, discounts and margin, a version history, and the client's answer. Later slices add the client's acceptance page with e-signature (portal), internal review before sending, and sales orders from accepted quotes.

## Decision

- **A quote belongs to the client** (like its opportunity) and optionally to an opportunity of that client. Its lines are stored in order; a **heading** groups the lines after it; an **item** has a quantity, unit, cost, price, discount and billing (one-time, monthly, quarterly, annual). An item from the catalog fills its blanks (SKU, name, unit, cost, price, billing) from the product; a bundle's price is its components' unless it has its own.
- **The whole quote is saved at once** (header and every line), with the version read, so the builder can edit freely and save once.
- **Totals** are worked out on read, never stored: one-time, recurring per month (quarterly ÷ 3, annual ÷ 12), first year (one-time + 12 months), its cost, the discounts given, and **margin on the first year**. **Options** count only when selected.
- **Revisions:** a quote is a draft until **sent**. Sending keeps a snapshot of the quote as sent (lines and totals) as its revision. Changing a sent quote puts it back to draft as the next revision, to be sent again. Sent quotes can't be deleted.
- **The answer:** a sent quote is **accepted** (naming the options taken; the rest are left out) or **declined** (with a reason), with who decided. Accepted and declined quotes can't change. Accepting moves its opportunity, if still open, to the first won stage.
- **PDF:** the API renders the quote (sections, options, totals, terms) with the same dependency-free PDF writer as dashboard exports. Branded templates come with the client acceptance page.
- **Events:** `sales.quote.created`, `.sent`, `.accepted`, `.declined`, with totals and margin in the payload.
- **Permissions** are the sales ones: `sales:read` to see, `sales:write` to change, send and record answers.

## Consequences

- Internal review (later) gates `send` and compares a resubmission against the last reviewed revision's snapshot.
- Sales orders (later) are made from accepted quotes' selected lines.
