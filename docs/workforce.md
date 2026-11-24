# Workforce

The **Workforce** module (`workforce`) describes your people:

- the skills they have, and at what level
- the certifications they hold, and when they expire
- what different kinds of work require of them

Dispatch uses it to send work to people who can do it. Availability and timesheets come later.

Turn it on under **Settings → Modules**.

## Skills

The skills catalog lists what your team can do, such as *Exchange Online*, *Intune* or *Firewalls*, in categories you choose. Each person has a level in a skill:

| Level | Meaning |
|---|---|
| 1 | Learning |
| 2 | Working |
| 3 | Proficient |
| 4 | Expert |

**Self-assessed or verified.** People set their own levels, and those are self-assessed. Someone with `workforce:manage` can verify a person's level, which records who verified it and when. If the person changes their level again, it goes back to self-assessed. Nobody can verify their own level.

## Certifications

The certifications catalog lists certifications by vendor, such as Microsoft *AZ-104* or Cisco *CCNA*. A certification can have a validity period in months.

**What people hold.** For each certification a person holds, Huxley records:

- when they earned it
- when it expires (worked out from the validity if you leave it out)
- a credential ID
- a link where it can be checked (`https://`)

People add their own certifications. Managers can add anyone's.

**States.** Each held certification shows as *valid*, *expiring* (within 30 days) or *expired*. Renewing one, by changing its dates, makes it valid again.

**Expiry events.** Huxley announces expiry so workflows can act on it, for example by reminding the person and their lead:

- `certification.expiring` once, 30 days before.
- `certification.expired` once, when it lapses.

The event names the person, the certification and its expiry date.

## What work requires

A queue or a ticket type can require:

- **Skills at a minimum level.** These are weighed: someone at *working* level in a skill that needs *proficient* meets two-thirds of that requirement.
- **Certifications.** These are hard requirements: the certification must be held and unexpired.

**Fit.** `GET /api/v1/workforce/fit?queueId=…&ticketTypeId=…` ranks people for a kind of work. People holding every required certification come first, then those whose skills best meet the required levels. Each person's entry lists what they're missing.

## Permissions

| Permission | Allows | Built-in roles |
|---|---|---|
| `workforce:read` | Seeing people's skills and certifications, and requirements | Technician, Read-only |
| `workforce:manage` | Keeping the catalogs, verifying skills, keeping anyone's records, setting requirements (instance-wide) | Owner, Admin |

Everyone can keep their own skills and certifications.

## API

| Method and path | What it does |
|---|---|
| `GET`, `POST /api/v1/workforce/skills`; `PUT`, `DELETE /api/v1/workforce/skills/{id}` | The skills catalog |
| `GET`, `POST /api/v1/workforce/certifications`; `PUT`, `DELETE …/{id}` | The certifications catalog |
| `GET /api/v1/workforce/people`, `GET …/people/{userId}` | People, and one person's skills and certifications |
| `PUT …/people/{userId}/skills/{skillId}` | Set a level, with `{ level, verified }`. A `null` level removes the skill |
| `POST …/people/{userId}/certifications`; `PUT`, `DELETE …/certifications/{id}` | Certifications someone holds |
| `GET`, `PUT /api/v1/workforce/requirements/{queue or ticket_type}/{id}` | What work requires |
| `GET /api/v1/workforce/fit` | Who fits a queue or ticket type, best first |
