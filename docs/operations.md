# Running Huxley

Notes for whoever runs a Huxley instance.

## Data retention

Some data grows without end, so a daily sweep removes what's older than its retention (ADR-085). Set the retentions in **Settings → Data retention** (`platform:retention:manage`).

| What | Kept for (default) | Notes |
| --- | --- | --- |
| Finished workflow runs | 12 months | With their step details and any payloads stored in object storage. Runs still running or waiting are never removed. |
| Delivered domain events | 30 days | Events workflows haven't picked up yet stay. |
| Read notifications | 180 days | Unread ones stay. |
| AI call records | 365 days | Usage and cost history. |

The sweep runs in the scheduler role a few minutes after start, then daily. **Sweep now** runs it straight away, and the page shows what the last sweep removed. Run partitions the sweep empties are dropped.

The **audit log** isn't swept. It's a hash chain (`huxley admin verify-audit` checks it), so export it to your SIEM or archive instead of deleting it.
