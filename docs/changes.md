# Change and problem management

Turn on **Change and problem management** (Settings → Modules; it's on in the service-desk starting point). A change is then a ticket of the **Change** type, with a **Change** card for its details (ADR-054).

## Filling one in

On a change ticket's page:

- **Type:** *standard* (pre-approved: approved as soon as it's submitted), *normal*, or *emergency* (may go ahead during a freeze).
- **Risk:** five questions about who would notice, downtime, testing, backout and familiarity. The answers score 0 to 13: **low** (up to 3), **medium** (up to 7) or **high**.
- **Implementation, backout and test plans.**
- **The window** it's planned for, and the **affected assets** (the client's). The card lists what else depends on them, from the [dependency map](assets.md).

## Submitting

**Submit for approval** checks the change:

- every risk question is answered and there's a window
- normal and emergency changes have implementation and backout plans

A normal or standard change whose window falls in a **freeze window** is refused. Move it, or make it an emergency. Other changes planned at the same time on the same assets are flagged when you submit, but don't block it.

## Approval

The built-in **Change approvals** pack decides submitted changes:

1. The **change advisory board** approves in Huxley. By default that's everyone with the Administrator role at the change's company or above. Change the workflow's approvers to your CAB team.
2. A **high-risk normal** change then goes to the **client's approvers**, who approve it in the [client portal](portal.md) (they're emailed).
3. The decision shows on the change, with who decided and their comment. The first rejection rejects it. A rejected change can be changed and submitted again.

Approval is an ordinary workflow, so change the policy there: by risk, type, client, or anything in the `change.submitted` event. The `core.change.decide` step records the outcome.

## After approval

- **Mark implemented** with how it went: successful, failed or backed out.
- Then write the **post-implementation review**.

## Changes and freezes

**Changes** in the sidebar lists the changes planned in the next four weeks, by day, with their risk and state, and the freeze windows. People with `servicedesk:configure` add and remove freezes there. A freeze set at an organization applies to it and the organizations below.

## Problems

A problem is a ticket of the **Problem** type, with a **Problem** card (ADR-055):

- **Incidents:** link them from the problem as **Incident (caused by this)**, or from an incident as **Problem (its cause)**. Each side shows the other.
- **Root cause and workaround.**
- **Known error:** tick it once the cause is known and there's a workaround. It raises `problem.known_error` with the root cause, workaround and count of linked incidents, so a workflow can tell the incidents' assignees or reply to their requesters.
- **Draft a knowledge article from the workaround** (with `kb:write`): a "Known error: …" article with the symptoms, cause and workaround, tagged `known-error`, linked to the problem and **submitted for review** in the [knowledge base](knowledge.md).

API: `GET`/`PUT /api/v1/tickets/{id}/problem` `{ rootCause, workaround, knownError }`, and `POST /api/v1/tickets/{id}/problem/article`. Link incidents with `POST /api/v1/tickets/{id}/links` `{ otherId, kind: "incident" }`.

## Events

`change.submitted`, `change.approved`, `change.rejected` and `change.implemented`. Each carries `reference`, `title`, `changeType`, `riskLevel`, `riskScore`, `plannedStart`, `plannedEnd`, `affectedAssets` (a count) and `state`.

## API

All under `/api/v1`:

- `GET`/`PUT /tickets/{id}/change` (`ticket:read`; `ticket:write` to change)
- `POST /tickets/{id}/change/submit`, `/implemented` `{ outcome }` and `/review` `{ review }` (`ticket:write`)
- `GET /tickets/{id}/change/impact`
- `GET /changes/questions` and `GET /changes/calendar?from=…&to=…`
- `GET`/`POST /change-freezes` and `DELETE /change-freezes/{id}` (`servicedesk:configure` to change)
