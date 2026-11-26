# Knowledge base

The knowledge base keeps how-tos, fixes and client documentation, reviewed before they're published. Turn on the **Knowledge base** module (`knowledge`) under **Settings → Modules**; it's part of the *Service desk* preset.

## Articles

An article belongs to an organization:

- **Your MSP's** for shared know-how, seen by everyone who may read the knowledge base there and below.
- **A client's** for that client's own documentation, seen only by those who may read it at that client.

Each article has a title, its text in markdown, an optional category, tags, and who can see it:

| Who can see it | |
|---|---|
| Internal | Technicians only (the default). |
| Clients | Also the organization's portal contacts, once the client portal arrives. |
| Public | Anyone on the portal, once it arrives. |

The text is markdown: headings, **bold**, `code`, lists, tables, task lists, footnotes and links. The editor's **Preview** shows it as readers will. For safety, raw HTML in an article is shown as text rather than run, and links may only go to `http(s)`, `mailto`, `tel` or relative addresses.

## Review and publishing

```
draft → in review → published
          ↓ rejected (with a comment): back to draft
```

- **Draft.** Only writers see it. Every saved change of the title or text is a new **revision**, and all of them are kept (**Revisions**).
- **Submit for review** when it's ready.
- **Approve and publish**, or **Reject** with what needs changing. The comment shows in the article's history for its author.
- **Published.** Readers see the published revision.
- **Editing a published article** makes a new draft beside it. Readers keep seeing the published revision until the draft is published in turn.
- **Archive** hides an article from readers. **Restore** brings it back: published again if nothing changed since, else as a draft.

Someone who may publish can also publish a draft directly, without a review.

**AI drafts:** the built-in AI auto-documentation pack drafts articles from resolved tickets, tagged `ai-draft`, linked to the ticket, and submits them for review here. They go through the same review as any article (see [Auto-documentation](servicedesk.md#auto-documentation)).

## Permissions

| Permission | Lets you |
|---|---|
| `kb:read` | Read published articles at the organization and below. |
| `kb:write` | Write articles, see drafts and revisions, and submit them for review. |
| `kb:publish` | Approve, reject, publish, archive and restore articles; change who can see a published article; keep categories. |

Technicians have `kb:read` and `kb:write`, and read-only users have `kb:read`. Who publishes is up to you: give `kb:publish` with a custom role. Owners and admins have everything.

## Search

**Knowledge** searches titles, tags and text, best match first, showing where each article matched. Readers search only what's published. Filter by category, and, if you write, by status.

## Search by meaning

**Knowledge → By meaning** finds published articles about the same thing as your words, even when they use other words: "a user can't sign in after changing their phone" finds *Reset MFA*. It uses the same setup as [searching tickets by meaning](servicedesk.md#search-by-meaning):

1. **pgvector** in the database.
2. **An embeddings provider** under **Settings → AI**.
3. **The Knowledge search by meaning pack** (`huxley/knowledge-search`): install it and publish *Embed articles*. Every 5 minutes it embeds newly published and republished articles (their published title and text), up to 200 a run.

A search is an AI call: logged, budgeted, and it needs `ai:use` at the organization you've picked as well as `kb:read`. Only published articles you may read are found, and drafts are never embedded.

## Articles on tickets

With the service desk on, a ticket's **Knowledge** card shows:

- **Linked articles:** what the ticket was solved with, or what to follow. Writers also see drafts linked there; readers only published articles.
- **Suggested:** published articles about the same thing as the ticket. When tickets and articles are embedded (both packs above), the closest in meaning come first, shown as *About the same thing (84%)*. This uses the ticket's own embedding, so it costs no AI call. Then come articles whose words best match the ticket's title and description, with where they matched. **Link** one to keep it with the ticket.
- **Find an article to link:** search the knowledge base from the ticket.

Linking needs `ticket:write` on the ticket and `kb:read` where the article is; archived articles can't be linked. An article's page lists the tickets it's linked to (for writers), so you can see where it's used.

## Categories

Publishers keep categories under **Knowledge → Categories**. A category belongs to an organization, and articles there and below can use it. Deleting a category leaves its articles uncategorized.

## Events

Workflows can react to articles:

| Event | When |
|---|---|
| `kb.article_created` | An article is written. |
| `kb.article_updated` | It's changed, or restored. |
| `kb.review_requested` | It's submitted for review. |
| `kb.review_rejected` | A review sends it back (`change.comment` says why). |
| `kb.article_published` | It's published (`change.reviewed` says whether it was reviewed). |
| `kb.article_archived` | It's archived. |
| `kb.article_linked`, `kb.article_unlinked` | It's linked to or unlinked from a ticket (`ticketId`, `reference`). |

Each carries the article's `title`, `status`, `visibility`, `revision` and the `actorId` of who made the change. For example, notify publishers when a review is requested.

## API

| Endpoint | |
|---|---|
| `GET /api/v1/kb/articles?q=…&categoryId=…&status=…` | Articles you can see, with `total` |
| `POST /api/v1/kb/articles` | Write one (`orgId`, `title`, `body`, `categoryId`, `visibility`, `tags`) |
| `GET`, `PATCH /api/v1/kb/articles/{id}` | Read one; change it (send the `version` you read) |
| `POST /api/v1/kb/articles/{id}/submit`, `/publish`, `/reject`, `/archive`, `/restore` | Move it through review (`publish` and `reject` take a `comment`) |
| `GET /api/v1/kb/articles/{id}/revisions`, `/revisions/{number}` | Its revisions |
| `GET`, `POST /api/v1/kb/categories`, `PUT`, `DELETE /api/v1/kb/categories/{id}` | Categories |
| `POST /api/v1/kb/preview` | Markdown rendered as an article shows it |
| `GET /api/v1/kb/tickets/{ticketId}/articles` | A ticket's linked and suggested articles, and whether you may link |
| `POST /api/v1/kb/tickets/{ticketId}/articles`, `DELETE …/articles/{articleId}` | Link an article (`articleId`); unlink it |
| `GET /api/v1/kb/articles/{id}/tickets` | The tickets an article is linked to |
| `GET /api/v1/kb/meaning?q=…&orgId=…` | Published articles by meaning: `{ available, message, hits: [{ id, title, orgName, categoryName, score }] }` |

An article read through the API has `bodyHtml` (the text rendered and made safe), `published` (what readers see), and `canWrite` and `canPublish` for you.

## Client documentation

A client's articles are its documentation: how its network, backups and line-of-business apps are set up and run. A client's **Documentation** tab lists them, and **New document** starts an article at that client. They follow the same review flow and permissions as any article, and are seen only by those with access to that client. Keep the client's passwords beside them in the [credential vault](vault.md).
