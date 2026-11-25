# Search

Press `Ctrl+K` (`⌘K` on a Mac), or click **Search or jump to…** in the top bar, to open the command palette. Start typing to find things, or to run a command such as *Go to Workflows*.

## What it finds

Type at least two characters. Huxley shows the best five matches of each kind, close matches included, so a typo still finds it:

| Kind | Matched on | Shown when |
|---|---|---|
| Tickets | Title, or reference (`T-1042` or `1042`) | The service desk is on and you can read tickets |
| Clients | Name | Clients and contacts is on and you can read them |
| Contacts | Name and email | The same |
| Workflows | Name and key | You can read workflows |
| People | Name and email | You can see people (Workforce) or users |

Search only finds what you're allowed to see: tickets, clients, contacts and workflows in the organizations where you hold the permission. Open tickets rank before resolved ones, and a ticket whose number you typed comes first.

To find tickets by what they're about rather than their words, use [Tickets → By meaning](servicedesk.md#search-by-meaning).

## API

`GET /api/v1/search?q=…` returns the same results, as `{ kind, id, title, subtitle }` objects.
