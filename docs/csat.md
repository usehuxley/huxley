# Satisfaction surveys

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

## API

- `GET /api/v1/tickets/{id}/csat`: a ticket's latest survey (`ticket:read`)
- Portal: `POST /api/portal/v1/tickets/{id}/rating` `{ rating, comment }`
- The answer link: `GET`/`POST /api/portal/v1/surveys/{token}`
