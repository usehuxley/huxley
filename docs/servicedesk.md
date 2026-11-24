# Service desk

The **Service desk** module (`servicedesk`) adds tickets: the requests and incidents your clients raise, worked by your technicians in service queues. It needs **Clients and contacts** (`crm`), which turns on with it.

Turn it on under **Settings → Modules**. A provider organization can turn it off for itself and its clients. With it off, nothing here exists for them.

## Tickets

A ticket belongs to the organization it's for, usually a client, and is visible to the people who can see that organization. Each has:

- **A type and a status.** Incident, Service request, Problem or Change, each with its own statuses.
- **A title and a description.**
- **Impact, urgency and priority.** See [Priority](#priority).
- **A queue, a requester and an assignee.** The requester is a contact at the ticket's organization. The assignee is a person, a team, or both.
- **Where it came from:** web, email, portal, API, workflow or phone.

**Numbers.** Tickets are numbered from one sequence across the instance, with the organization's ticket prefix (**Settings → Organization settings → Numbering**), for example `ACME-1042`. A ticket keeps its number if the prefix changes later.

**Editing.** Edits send back the `version` that was read. If someone changed the ticket in between, the edit is refused with `409` rather than overwriting their change.

## Statuses

Every status belongs to one of five categories: `new`, `open`, `pending`, `resolved` or `closed`. Views, SLAs and reports work with the category, so a status can be named anything.

Incidents and service requests start with this workflow:

| Status | Category | Notes |
|---|---|---|
| New | new | Where tickets start |
| In progress | open | |
| Waiting on client | pending | Pauses the SLA clock |
| Waiting on third party | pending | Pauses the SLA clock |
| Resolved | resolved | Needs a resolution note |
| Closed | closed | |

**What a status needs.** A status can require something before a ticket can enter it:

- **A resolution note:** a ticket can only move to **Resolved** if it already has one, or if one is given with the change.
- **An assignee:** the ticket must be assigned.

**Allowed changes.** A type can limit which status may follow which. A type with no limits allows any change.

**Timestamps.** Moving a ticket to a `resolved` or `closed` status records when it was resolved, and closing it records when it was closed. Reopening it clears both.

## Priority

Impact and urgency each run from 1 (high) to 3 (low). Together they set the priority:

| Impact \ Urgency | 1 | 2 | 3 |
|---|---|---|---|
| **1** | P1 | P2 | P3 |
| **2** | P2 | P3 | P4 |
| **3** | P3 | P4 | P4 |

You can choose a priority yourself instead. Your choice sticks when impact or urgency change later. Setting the priority to `auto` makes it follow the table again.

## Notes and history

Notes are one of three kinds:

- **Public:** the requester will see it, through the portal and email once those arrive.
- **Internal:** for your team only.
- **Resolution:** how the ticket was resolved.

The first public note from a person sets the ticket's **first response** time.

**History.** Every change appears on the ticket's activity timeline and in the audit log. Changes also emit events that workflows can be triggered by: `ticket.created`, `ticket.updated`, `ticket.status_changed`, `ticket.assigned` and `ticket.note_added`.

## Queues

A service queue is where tickets wait to be picked up. It belongs to an organization, usually your MSP, and its members are people and teams.

Setting up queues needs `servicedesk:configure`. Archiving a queue removes it from lists and from new tickets. Tickets already in it stay there until they're moved.

## Permissions

| Permission | Allows | Built-in roles |
|---|---|---|
| `ticket:read` | Seeing tickets, their notes and their activity | Technician, Read-only |
| `ticket:write` | Creating and working tickets: editing, notes, status, assignment | Technician |
| `servicedesk:configure` | Setting up queues (instance-wide) | Owner, Admin |

## API

| Method and path | What it does |
|---|---|
| `GET /api/v1/tickets` | List tickets. Filter with `orgId`, `queueId`, `assignee` (a person's id, `me` or `none`), `state` (`open`, the default, or `resolved`, `closed` or `all`) and `search` (title or number) |
| `POST /api/v1/tickets` | Create a ticket |
| `GET /api/v1/tickets/{id}` | Read a ticket |
| `PATCH /api/v1/tickets/{id}` | Edit a ticket. A field you leave out is unchanged. `null` clears the queue, requester or assignee |
| `POST /api/v1/tickets/{id}/status` | Change the status, with a `resolution` where it's needed |
| `GET`, `POST /api/v1/tickets/{id}/notes` | Read or add notes |
| `GET /api/v1/tickets/{id}/activity` | Read the timeline |
| `GET`, `POST /api/v1/queues`, and `GET`, `PUT`, `DELETE /api/v1/queues/{id}` | Read and manage queues |
| `GET /api/v1/servicedesk/types` | Read ticket types, their statuses and allowed changes |
| `GET /api/v1/servicedesk/priority-matrix` | Read the priority table |

## Coming next

The ticket screens in the web app, SLAs, email intake, time entries, merging and linking tickets, templates, saved views, and workflow steps for tickets.
