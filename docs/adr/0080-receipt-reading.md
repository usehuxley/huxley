# ADR-080: AI reads receipts into a suggestion the person confirms

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

§8.16: AI reads a receipt's amount, date and vendor for the person to confirm. Huxley's AI calls so far took text only.

## Decision

- **AI requests can carry one file**, a photo or a PDF, sent before the prompt:
  - Anthropic: an `image` or `document` block.
  - OpenAI-style providers: `image_url` or `file`, as a data URL.
- **Keeping a receipt raises `expense.receipt_added`**, and the built-in **Receipt reading** pack runs `core.expense.read_receipt`.
  - The step loads the receipt from object storage (up to 10 MB) and asks the organization's AI provider for the total, tax, currency, date, vendor and a short description.
  - The answer is structured, with null for anything not read with confidence.
  - The call is logged against the run, as AI steps are.
- **The reading is kept beside the entry** (`reading`, or `reading_error`), and never applied by itself. **Use these** applies it, on the person's own draft:
  - It takes the amount, tax, currency, a valid date within the past year, and the vendor.
  - It keeps whatever wasn't read, and audits the change.
- **A new receipt clears the old reading.**

## Consequences

- Other features can send files to AI the same way, for example document intake.
- Receipt contents go to the organization's AI provider. An organization without one gets no reading, and its entries are unchanged.
