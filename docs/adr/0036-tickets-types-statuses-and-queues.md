# ADR-036: Tickets, with per-type statuses, service queues and a priority matrix

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-25

## Context

The `servicedesk` module (design doc §8.2) is the heart of Phase 5. Its tickets need:

- types (Incident, Service Request; Problem and Change come later)
- a status workflow per type
- service queues rather than boards
- priority from impact and urgency
- public and internal notes
- human-friendly numbers

The core has to come first, so that SLAs, mailbox intake, dispatch and the UI can build on it.

## Decision

### Configuration (instance-wide)

**Ticket types** (`sd_ticket_types`) each have their own **statuses** (`sd_statuses`).

- **Categories:** every status has one of `new`, `open`, `pending`, `resolved` or `closed`. Categories are what SLAs, views and reports reason about, so a status's name can be anything.
- **SLA pauses:** a status can pause the SLA clock (`pauses_sla`), as *Waiting on client* does.
- **Requirements on entry:** a status can require things before a ticket enters it (`requires`). For now, `resolution` means a resolution note must exist or be given with the change, and `assignee` means the ticket must be assigned.
- **Transitions:** a type may restrict which status can follow which (`sd_transitions`). A type without transition rows allows any change.
- **Seeded types:** Incident, Service request, Problem and Change. The first two share a default workflow: New → In progress → Waiting on client / Waiting on third party → Resolved (needs a resolution) → Closed.

**The priority matrix** (`sd_priority_matrix`) maps impact (1 high … 3 low) × urgency (1 … 3) to a priority from P1 to P4.

- **When it applies:** a ticket's priority follows the matrix unless someone sets it directly.
- **A direct choice sticks:** once a person sets a priority, it stays put until they change it again.

### Records (belong to an organization, with row-level security)

**Service queues** (`sd_queues`) belong to an organization, usually the MSP, and have **members**: people and teams. Queues are where tickets wait to be picked up.

**Tickets** (`sd_tickets`) belong to the client organization they're for. Each ticket has:

- type and status
- title and description
- impact, urgency and priority
- queue, requester contact (from `crm`) and assignee (a person or a team)
- source, custom fields, and timestamps: resolved, closed, first response

**Ticket numbers** come from one instance-wide sequence, shown with the organization's ticket prefix (`numbering.ticket_prefix`, e.g. `ACME-1042`). The prefix is stored when the ticket is created, so a ticket's number never changes.

**Notes** (`sd_ticket_notes`) are public (visible to the client later, through the portal and email), internal, or resolution notes. The first public note from staff sets the first-response time.

**History:** every change is recorded in the ticket's **activity** (`sd_ticket_activity`), which is its timeline, as well as in the audit log. Changes also emit domain events (`ticket.created`, `ticket.updated`, `ticket.status_changed`, `ticket.assigned`, `ticket.note_added`), so workflows can react.

### Permissions

- **`ticket:read`:** view tickets.
- **`ticket:write`:** create and work tickets, add notes, change status, and assign.
- **`servicedesk:configure`** (instance-wide): manage queues.

Technicians get `ticket:read` and `ticket:write`; Read-only gets `ticket:read`. Every route checks that the module is active at the ticket's organization.

## Consequences

- **Built on next:**
  - SLAs: policies, business hours from site calendars, pauses, warnings and breaches.
  - Mailbox intake, time entries, relations and merges, templates and canned responses, and saved views.
- **Configuring types and statuses:** editing them through the API and UI comes later. Until then the seeded set is what's available.
