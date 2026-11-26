# Workforce

The **Workforce** module (`workforce`) describes your people:

- the skills they have, and at what level
- the certifications they hold, and when they expire
- what different kinds of work require of them

It also records when people are available: their working hours and time off. [Dispatch](servicedesk.md#dispatch) uses all of this to send work to people who can do it and are there to do it. Timesheets come later.

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

## Availability

**Working hours.** A person's hours per weekday, in their time zone. People set their own hours, and managers can set anyone's. Someone without hours set counts as available at any time.

**Time off.** Each booking has dates (inclusive), a kind (vacation, sick, training or other) and a note.

- **Your own:** you request it, and a manager approves or declines it. Nobody decides their own.
- **Someone else's:** a manager booking it approves it at once.
- **Cancelling:** you can cancel time off that hasn't ended.
- **Events:** each step emits an event, so workflows can notify people or update calendars. The events are `time_off.requested`, `time_off.approved`, `time_off.declined` and `time_off.cancelled`.

**Where it shows.** The **People** page marks who is away today. Managers also see the requests waiting for them there.

## Timesheets

A **timesheet** is a person's time for a period, weekly from Monday by default. **My time** shows each week's timesheet:

- each day's logged time against the hours their working schedule expects (time off counts as expected-free)
- entries without notes
- the timesheet's status and history

**The flow:**

1. **Submit.** At the end of the period, the person submits it with an optional note for their lead. While it's submitted, their time entries on its days can't be added, changed or deleted. They can **withdraw** it to make changes.
2. **Review.** Their reviewer opens it from **Timesheets** in the sidebar and sees the days, entries and history.
   - **Approve:** the time stays locked.
   - **Reject:** needs a comment saying what to change. The time can change again, and they resubmit.
3. **Reopen.** A reviewer can reopen a submitted or approved timesheet, with a reason. It goes back to open and through review again. Everything is recorded in the timesheet's history and the audit log.

**Who reviews:**
- The person's **team lead**: a member marked **Lead** on the team's settings page, holding `timesheet:approve`.
- For people no one leads, anyone with `workforce:manage`.
- Nobody reviews their own.

**Events:** `timesheet.submitted`, `timesheet.approved`, `timesheet.rejected`, `timesheet.reopened` and `timesheet.withdrawn`, so workflows can notify people.

**Settings** (on **Timesheets**, with `workforce:manage`):
- **Periods:** weekly, or every two weeks, from a day you choose.
- **Bill only approved time:** billing previews, an [agreement's usage](agreements.md), then count only time on approved timesheets. Allowances are still used up by all covered time, in order. The usage API also takes `approvedOnly` directly.

**Reminders:** the built-in **Timesheet reminders** pack (`huxley/timesheet-reminders`) adds three workflows. Install it at your MSP and publish them. They're ordinary workflows, so you can change their schedules, wording and rules.
- **Remind people to submit their timesheets:** every weekday at 9:00, it tells people whose last timesheet isn't submitted, or was rejected and not resubmitted. From three days late, it tells their team leads too.
- **Remind people about days logged short:** every morning, it tells people who logged less yesterday than their working hours expect, unless they were on time off.
- **Remind people to add notes to their time:** on Fridays, it tells people how many of this period's entries have no notes.

They use the `core.timesheet.check` step: see [the step reference](workflows.md).

**Payroll export:** **Timesheets → Payroll export** downloads a CSV for a date range. It holds approved timesheets' time by person, day and work type, and approved time off as each scheduled day's hours (8 hours on weekdays without a schedule).

## What work requires

A queue or a ticket type can require:

- **Skills at a minimum level.** These are weighed: someone at *working* level in a skill that needs *proficient* meets two-thirds of that requirement.
- **Certifications.** These are hard requirements: the certification must be held and unexpired.

**Fit.** `GET /api/v1/workforce/fit?queueId=…&ticketTypeId=…&at=…` ranks people for a kind of work at a moment (`at`, default now):

1. People holding every required certification.
2. Then people who are available: not on approved time off that day, and within their working hours.
3. Then by how well their skills meet the required levels.

Each person's entry lists what they're missing, and why they're unavailable (`away` or `off_hours`).

## Permissions

| Permission | Allows | Built-in roles |
|---|---|---|
| `workforce:read` | Seeing people's skills and certifications, and requirements | Technician, Read-only |
| `workforce:manage` | Keeping the catalogs, verifying skills, keeping anyone's records, setting requirements (instance-wide); reviewing timesheets of people no one leads, timesheet settings, the payroll export | Owner, Admin |
| `timesheet:submit` | Seeing and submitting your own timesheets | Technician |
| `timesheet:approve` | Reviewing the timesheets of people in teams you lead | Owner, Admin |

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
| `GET`, `PUT …/people/{userId}/schedule` | Working hours. `null` removes them |
| `POST …/people/{userId}/time-off`; `GET /api/v1/workforce/time-off` | Request or book time off, and list it (by `userId` and `status`) |
| `POST /api/v1/workforce/time-off/{id}/decision`, `…/cancel` | Approve or decline it, with `{ approve, note }`; cancel it |
| `GET /api/v1/timesheets/mine?date=` | Your timesheet for the period containing a day |
| `POST /api/v1/timesheets/submit` | Submit yours: `{ periodStart, note }` |
| `GET /api/v1/timesheets?status=`; `GET …/{id}` | Timesheets you review (submitted by default), and one timesheet |
| `POST …/{id}/decision`; `POST …/{id}/reopen` | Approve or reject (`{ approve, note }`); reopen or withdraw (`{ note }`) |
| `GET`, `PUT /api/v1/timesheets/settings`; `GET …/payroll.csv?from=&to=` | Settings, and the payroll export |
| `PUT /api/v1/teams/{id}/members/{userId}` with `{ lead }` | Make a member a team lead, or not |
