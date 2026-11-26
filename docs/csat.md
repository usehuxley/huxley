# Satisfaction and net promoter surveys

The **Customer experience** module asks requesters how it went when their ticket is resolved (ADR-043). Answers show on the ticket, in dashboards, and as events for workflows.

## Turning it on

1. Turn on the **Customer experience** module (Settings → Modules).
2. Install the built-in **Satisfaction surveys** pack (Settings → Packs) and publish its two workflows:
   - **Ask how it went** runs when a ticket is resolved.
   - **Tell the technician about a poor rating** runs when someone answers 1 or 2, or thumbs down.
3. Emails need email set up and the portal's address: the company's portal domain, or `HUXLEY_HTTP__PORTAL_PUBLIC_URL`.

## Who's asked

The ticket's requester, unless:

- **their company opted out:** Satisfaction surveys is off in its settings (Customer experience group);
- **sampling leaves the ticket out:** *Survey this share of tickets* asks about that percentage of resolved tickets, at random;
- **they were asked about it in the last week**, for example when a ticket is reopened and resolved again;
- **the ticket has no requester.**

These settings are inherited down the organization tree and can be locked, like other organization settings. *Thumbs up or down* asks for a thumbs up or down instead of a rating from 1 to 5.

To ask only about some tickets (a queue, a type, after a delay…), edit the **Ask how it went** workflow, e.g. with a filter on its trigger.

## Answering

- **From the email:** one link per rating. The link opens a page with that rating chosen, where they can add a comment and press **Send**. Opening a link alone doesn't answer, because email security scanners open links too.
- **In the portal:** on the resolved ticket, under *Your rating*.
- **One answer, within 30 days.**

## Seeing the answers

- **On the ticket:** the *Satisfaction* card shows the rating, the comment and how it was answered, or that it's waiting.
- **Dashboards:** the prebuilt **Customer satisfaction** dashboard shows the satisfaction score and its trend, and splits it by technician and by client. The `csat` [reporting source](analytics.md) has `satisfaction` (100 for a 4 or 5, or a thumbs up; 0 otherwise; empty when unanswered), so its average is the CSAT score.
- **Workflows:** the `csat.responded` event carries `rating`, `scale`, `positive`, `comment`, `channel` (email or portal), `contactId` and `assigneeUserId`.

## In workflows

`core.csat.survey` with `{ ticket }` asks about a resolved ticket. Its output is `{ sent, skipped, survey, emailed }`, where `skipped` says why it didn't ask: `opted_out`, `no_requester`, `already_asked`, `sampled_out` or `not_resolved`.

## Net promoter surveys

Every so often, ask your clients' people how likely they are to recommend you, from 0 to 10.

- **Answers:** promoters answer 9 or 10, passives 7 or 8, and detractors 0 to 6. The **NPS** is promoters' share minus detractors', from −100 to 100.
- **Turning it on:** install the built-in **Net promoter surveys** pack and publish its workflows.
  - **Ask the net promoter question** runs on the first day of each quarter (or by hand).
  - **Tell account managers about a detractor** notifies administrators when someone answers 0 to 6.
- **Who's asked:** contacts with an email address who raised a ticket lately (*Ask people who raised a ticket within*, default 180 days), and weren't asked lately (*Ask the same person at most every*, default 90 days), unless their company turned off *Net promoter surveys*. These are Customer experience settings, inherited down the organization tree.
- **Answering:** by one-click links in the email (the page records the score when they press **Send**), or on the portal's home page. One answer, within 30 days.
- **Seeing the answers:**
  - the contact's **Net promoter** card
  - the prebuilt **Net promoter score** dashboard (the score, the promoter/passive/detractor mix, and the score by client)
  - the `nps` reporting source: its `nps` field is 100, 0 or −100, so its average is the NPS
  - the `nps.responded` event, which carries `score`, `category`, `comment`, `channel` and `campaign`
- **In workflows:** `core.nps.send` asks the contacts of the run's organization and below. It takes an optional `campaign` (default: the month) and `limit` (default 500), and outputs `{ asked, emailed, campaign }`. Save a workflow at one client to ask only its people.

## API

- `GET /api/v1/tickets/{id}/csat`: a ticket's latest survey (`ticket:read`)
- Portal: `POST /api/portal/v1/tickets/{id}/rating` `{ rating, comment }`
- The answer link: `GET`/`POST /api/portal/v1/surveys/{token}`
- NPS: `GET /api/v1/contacts/{id}/nps` (`crm:read`); portal `GET`/`POST /api/portal/v1/me/nps` `{ score, comment }`; the answer link `GET`/`POST /api/portal/v1/nps/{token}`
