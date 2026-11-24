# Service desk

The **Service desk** module (`servicedesk`) adds tickets: the requests and incidents your clients raise, worked by your technicians in service queues. It needs **Clients and contacts** (`crm`), which turns on with it.

Turn it on under **Settings → Modules**. A provider organization can turn it off for itself and its clients. With it off, nothing here exists for them.

## Tickets

**Tickets** in the sidebar lists them by view (assigned to you, unassigned, all open, resolved, closed), with search and a queue filter. A ticket's page shows its conversation: replies, internal notes and every change, oldest first. Its status and properties are alongside.

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

## SLAs

An SLA policy sets targets for each priority: how long until the first response, and until the ticket is resolved. Both are measured in business time.

**Business hours.** A policy counts one of these:

- **The client's site hours** (the default): the hours and timezone of the client's primary site. If that site has no hours, the policy's own hours apply.
- **The policy's own hours**, in its timezone. Without one, it uses the organization's time zone setting.
- **Around the clock.**

**Which policy applies.** A policy can be set on any organization and applies to the organizations below it too. The nearest one wins. Without one, the default policy applies: the seeded **Standard** policy.

| Priority | First response | Resolution |
|---|---|---|
| P1 | 30 minutes | 4 hours |
| P2 | 1 hour | 8 hours |
| P3 | 4 hours | 24 business hours |
| P4 | 8 hours | 40 business hours |

**How the clocks run:**

- **Fixed terms.** A ticket keeps the terms it was opened under. Changing a policy affects new tickets only.
- **Priority changes.** Changing a ticket's priority switches it to that priority's targets.
- **First response.** This clock runs from creation, and the first public reply meets it. It doesn't pause.
- **Resolution.** This clock pauses in waiting statuses (*Waiting on client*, *Waiting on third party*), stops when the ticket is resolved, and resumes if it's reopened.

**Warnings and breaches.** At 75% of a target, and again when the target is missed, Huxley adds a line to the ticket's timeline. It also emits an event that workflows can be triggered by:

- `ticket.sla_warning` for a warning.
- `ticket.sla_breached` for a missed target.

The event's `clock` field is `response` or `resolution`.

**Setting up.** Managing policies and choosing them for organizations needs `servicedesk:configure`:

- `GET`, `POST /api/v1/servicedesk/sla-policies`, and `PUT`, `DELETE /api/v1/servicedesk/sla-policies/{id}` manage policies.
- `GET`, `PUT /api/v1/orgs/{id}/sla` reads or sets an organization's policy. Setting `null` makes it follow the one above.

## Notes and history

Notes are one of three kinds:

- **Public:** the requester will see it, through the portal and email once those arrive.
- **Internal:** for your team only.
- **Resolution:** how the ticket was resolved.

The first public note from a person sets the ticket's **first response** time.

**History.** Every change appears on the ticket's activity timeline and in the audit log. Changes also emit events that workflows can be triggered by: `ticket.created`, `ticket.updated`, `ticket.status_changed`, `ticket.assigned` and `ticket.note_added`.

## Queues

A service queue is where tickets wait to be picked up. It belongs to an organization, usually your MSP, and its members are people and teams.

Set them up under **Settings → Service queues**, which needs `servicedesk:configure`. Archiving a queue removes it from lists and from new tickets. Tickets already in it stay there until they're moved.

## Email

Mail sent to a service desk mailbox becomes tickets and replies.

### Mailboxes

A mailbox belongs to an organization, usually your MSP, and has an address. Its rules say what happens to its mail:

- **New tickets:** their type and queue.
- **Unknown senders:** what happens when the sender isn't a contact:
  - **Create a contact** (the default). The contact goes at the company whose contacts share the sender's email domain, or else at the mailbox's organization for unknown senders. Personal-mail domains such as gmail.com don't count.
  - **Open the ticket without a requester**, in that organization.
  - **Ignore the message.**
- **Strip quoted replies and signatures** (on by default).

**How mail arrives.** There are two ways.

**Fetched over IMAP.** For an `imap` mailbox, give the server (`host`, `port`, `security`, `username`, `folder`) and a password. The password is encrypted with your organization's key and is never shown again; leave it out when saving to keep it.

- **Security:** `tls` (the default, port 993), `start_tls` (port 143), or `none` for a server on your own network.
- **Folder:** `INBOX` by default.
- **Polling:** the *Poll mailboxes* workflow fetches every minute.
- **What gets fetched:**
  - The first time, it takes the folder's unread messages, not its whole history.
  - After that, it takes whatever is new, up to 50 messages per mailbox per run.
- **Read flags:** fetched messages are marked read.
- **Errors:** if a mailbox can't be reached (a wrong password, say), its error is kept on the mailbox, and the other mailboxes are still fetched.

**Posted to Huxley.** For a `forward` mailbox, mail arrives by being posted to it, the way a mail server's forwarding rule or a mail service's inbound webhook would:

```sh
curl -X POST https://huxley.example.com/api/v1/mailboxes/<id>/messages \
  -H "Authorization: Bearer $HUXLEY_TOKEN" -H "Content-Type: message/rfc822" \
  --data-binary @message.eml
```

Posting needs `mailbox:ingest`, so give that to a service account used only for this.

**What Huxley keeps.** Each message is kept once, by its `Message-ID`: the raw message in object storage, and what Huxley needs from it in the database. Huxley then emits `email.received` in the mailbox's organization.

### Email to ticket

Set it up in four steps:

1. Install the built-in pack **Service desk email** at your MSP, under **Settings → Packs → Built-in packs**.
2. Publish its workflows. Like any pack's workflows, they arrive as drafts.
3. Add a mailbox under **Settings → Mailboxes**.
4. Send it an email.

The pack (`huxley/servicedesk-email`) adds three workflows:

- *Poll mailboxes* fetches IMAP mail every minute.
- *Email to ticket* turns each email into a ticket or a reply (below).
- *Email replies* emails your public notes to the requester (see [Email replies](#email-replies)).

*Email to ticket* runs the `core.ticket.from_email` step for each email, which does this:

1. **Skips automatic mail:** out-of-office and other auto-replies, bounces, and list or bulk mail.
2. **Skips loops:** mail from one of Huxley's own mailboxes, or more than 20 messages from one sender to a mailbox in an hour.
3. **Threads replies.** It looks for the `[HX#1042]` token in the subject first, then the messages the email answers (`In-Reply-To` and `References`).
   - **An open ticket:** the reply becomes a public note from the sender.
   - **A resolved ticket:** it reopens, with the reply as a note.
   - **A closed ticket:** the reply opens a new ticket instead.
4. **Opens a ticket** for the sender's company if the email isn't a reply. The subject, without `Re:` or `Fwd:`, becomes the title, and the text becomes the description. Its source is *email*, and the SLA applies as usual.
5. **Attaches files.** The email's attachments go on the ticket. Images shown in the text don't.

What became of each email is recorded on it: `GET /api/v1/mailboxes/{id}/emails` lists a mailbox's latest emails. The workflow is an ordinary one: copy it to add steps, for example to triage new tickets with AI.

Mailboxes are managed with `servicedesk:configure`:

- `GET`, `POST /api/v1/mailboxes`
- `GET`, `PUT`, `DELETE /api/v1/mailboxes/{id}`

### Email replies

A public note written on a ticket is emailed to the ticket's requester by the *Email replies* workflow.

- **Who it's from:** the mailbox the ticket's mail came to. If no mail came, it's the nearest mailbox at the ticket's organization or above.
- **The subject:** it carries the ticket's `[HX#1042]` token.
- **Threading:** the message continues the email thread (`In-Reply-To`, `References`), so the requester's answer comes back to the same ticket, even if their mail client drops the token.
- **The text:** your note, your name and the mailbox's name, and a line saying that replying adds to the ticket.
- **What isn't emailed:** internal and resolution notes, notes that came from the requester by email, and notes already sent. A ticket without a requester address isn't emailed either.

Each note shows whether it was emailed, and to whom, or why sending failed. A failed send is retried by the workflow.

The email goes out through the instance's mail server (`email.smtp_url`), with the mailbox's address as the sender and as **Reply-To**. That server must be allowed to send as the mailbox's address. Otherwise the message may be rejected or marked as spam, so set up SPF and DKIM for that domain accordingly.

**Tickets as records.** Tickets now also take attachments, custom fields and tags, like organizations and contacts do.

## Permissions

| Permission | Allows | Built-in roles |
|---|---|---|
| `ticket:read` | Seeing tickets, their notes and their activity | Technician, Read-only |
| `ticket:write` | Creating and working tickets: editing, notes, status, assignment | Technician |
| `servicedesk:configure` | Setting up queues, SLA policies and mailboxes (instance-wide) | Owner, Admin |
| `mailbox:ingest` | Posting email to mailboxes (instance-wide) | Owner, Admin |

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

Microsoft 365 mailboxes, sending through a mailbox's own server, mailbox screens, holidays in business hours, time entries, merging and linking tickets, templates, saved views, and workflow steps for tickets.
