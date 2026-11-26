# ADR-052: Reports export without new dependencies, and schedule as workflows

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-26

## Context

Phase 8's report builder needs scheduled reports and CSV/XLSX/PDF export. Two questions:

1. **Formats:** spreadsheet and PDF libraries are large, and PDF ones usually need embedded fonts.
2. **Scheduling:** a scheduled report needs a scheduler, retries, history, and a way to pause or change it.

## Decision

**Export is written in `huxley-analytics::export`.** Each widget's answer becomes a titled table, rendered from the same tables in three formats:

- **CSV:** a block per widget. Values are quoted, and text that looks like a formula is defused.
- **XLSX:** a minimal Office Open XML workbook, a sheet per widget, with numbers as numbers. It's built with the `zip` crate the workspace already has.
- **PDF:** a minimal PDF 1.4 writer using the built-in Helvetica fonts, so there are no font files. Tables are laid out on A4 pages, and text outside Latin-1 becomes `?`.

**Charts export as their tables.** The numbers are the point, and every chart already has a table view.

**Access:**
- `GET /api/v1/dashboards/{id}/export?format=…&range=…&orgId=…` runs each widget with the viewer's own access, optionally narrowed to one organization. Widgets they can't see say so.
- The web app's **Download** menu streams it.

**Scheduled reports are workflows.**

- `core.report.send` renders a shared or prebuilt dashboard with the numbers of the run's organization and below. It emails the file as an attachment to people only: `user:`, `team:` or `role:`, resolved like `core.notify`. Email gained attachments for this.
- The dashboard's **Email regularly** creates and publishes a workflow: a schedule trigger (daily, Mondays or the 1st, at an hour, in the organization's time zone) and that step.
- Scheduling, retries, run history, pausing and editing are then the workflow engine's.

## Consequences

- No new crates, so no licence review and no binary bloat. The PDF is plain but readable, and XLSX opens in Excel, LibreOffice and Google Sheets.
- A scheduled report shows the organization's numbers to its recipients, so whoever schedules it decides who sees them. Only people with accounts can receive it.
- Personal dashboards can't be scheduled, because a workflow runs at an organization, not as a person.
