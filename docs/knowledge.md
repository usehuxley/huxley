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

## Permissions

| Permission | Lets you |
|---|---|
| `kb:read` | Read published articles at the organization and below. |
| `kb:write` | Write articles, see drafts and revisions, and submit them for review. |
| `kb:publish` | Approve, reject, publish, archive and restore articles; change who can see a published article; keep categories. |

Technicians have `kb:read` and `kb:write`, and read-only users have `kb:read`. Who publishes is up to you: give `kb:publish` with a custom role. Owners and admins have everything.

## Search

**Knowledge** searches titles, tags and text, best match first, showing where each article matched. Readers search only what's published. Filter by category, and, if you write, by status.

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

An article read through the API has `bodyHtml` (the text rendered and made safe), `published` (what readers see), and `canWrite` and `canPublish` for you.

## Coming next

Linking articles to tickets, suggested articles on a ticket, and search by meaning.
