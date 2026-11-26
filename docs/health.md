# Client health

Each client company gets a daily health score from 0 to 100, with the reasons behind it, so account managers hear about an unhappy client early (ADR-044).

## Turning it on

1. Turn on the **Customer experience** module.
2. Install the built-in **Client health** pack and publish its workflows:
   - **Score client health** scores each client every morning at 06:00 (or when run by hand).
   - **Tell account managers about a declining client** notifies administrators when a client's score drops 10 points or more in a week.

## The score

| Factor | Based on |
|---|---|
| Satisfaction surveys | The share of satisfied answers in 90 days ([satisfaction surveys](csat.md)) |
| Net promoter score | The NPS in 180 days |
| Client sentiment | The average tone of the client's messages in 30 days, judged by AI (see below) |
| SLA targets met | The share of tickets that missed an SLA target in 90 days |
| Tickets staying resolved | The share of resolved tickets that were reopened in 90 days |
| Ticket volume | Tickets in the last 30 days against the usual 30 days |
| Agreement renewals | An agreement ending within 60 days without renewing automatically |

A factor without enough data (a few survey answers, a handful of tickets) doesn't count either way. The score is the weighted average of the rest. With none, the client shows **Not enough data**.

- **Weights:** 0 to 10 per factor, on the organization's settings page under **Client health model** (`crm:manage`). They apply to its clients and those below, unless set again further down. 0 leaves a factor out.
- **Healthy** is 75 or more, **Needs attention** 50 to 74, and **At risk** under 50.

## Sentiment

Install the built-in **AI sentiment** pack (and set up an [AI provider](ai.md)) to judge the tone of every client message: tickets raised by clients and their public replies, by email or in the portal (ADR-046).

- **Judging:** each message gets a score from -2 (very negative) to 2 (very positive) and a one-line reason. The ticket page's **Sentiment** card shows them. The workflow *Judge the tone of client messages* holds the prompt and rubric, so you can change them, and each call is in the AI log.
- **Falling sentiment:** when a client's last 30 days average 0.75 or more below the 60 days before (with at least 3 messages in each), the daily score raises `client.sentiment_declining` with `recent`, `previous` and `explanations`: the most negative recent messages, as "T-123: reason". The pack tells administrators. A client is flagged at most once a week.
- **Reports:** the `sentiment` [reporting source](analytics.md) has every judged message.

## Seeing it

- **The client page:** the **Health** card shows the score, its trend over 90 days, the reasons (factors under 60, worst first), and every factor with its basis.
- **Reports:** the `health` [reporting source](analytics.md) has each client's daily score.
- **Workflows:** `client.health_declining` carries `name`, `score`, `previous` and `reasons`. Use it to open a task, alert an account manager, or schedule a call.

## In workflows

`core.health.compute` scores the client companies of the run's organization and below for today. Running it again the same day replaces that day's snapshot. Its output is `{ companies, declining: [{ org_id, name, score, previous, reasons }], sentiment_falling: [{ org_id, name, recent, previous, explanations }] }`.

`core.sentiment.record` stores a message's sentiment: `ticket_id`, `note_id` (empty for the ticket's description), `score` (-2 to 2) and `reason`.

## API

- `GET /api/v1/orgs/{id}/health` (`crm:read`): the latest score, factors, reasons, and 90 days of history
- `GET`/`PUT /api/v1/orgs/{id}/health-model` `{ weights }` (`crm:read`, `crm:manage` to set)
- `GET /api/v1/tickets/{id}/sentiment` (`ticket:read`): a ticket's judged messages
