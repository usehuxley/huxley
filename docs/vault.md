# Credential vault

The credential vault keeps your clients' admin logins, API keys, Wi-Fi passwords and secret notes next to their documentation. It's optional: turn on the **Credential vault** module (`vault`) under **Settings → Modules**. It isn't part of any preset.

## Passwords on a client

A client's **Passwords** tab lists its items: name, kind (login, API key, Wi-Fi, secret note, other), username, address, notes and tags. The secret itself is never on the page until you reveal it.

- **Reveal** asks for an optional reason, such as the ticket you're working. The secret then shows for 30 seconds, with **Copy**, and is hidden again.
- **Every reveal is recorded:** who, when and why. Each item shows when it was last revealed. The record stays even if the item is deleted.
- **Edit** to change the details or set a new secret. Leaving the secret empty keeps the current one, and **Remove the secret** clears it.

## How secrets are kept

- **Encrypted at rest** with the organization's own data key: the same envelope encryption as connection credentials and secret variables (design doc §11). Each secret is bound to its item, so it can't be moved to another one.
- **Never listed:** lists, item reads, the audit log and events never include it. Only a reveal returns it, and that response is marked not to be cached.
- **Audited:** creating, changing and deleting items, and every reveal, are audit events (`vault.item_created`, `vault.item_updated`, `vault.item_deleted`, `vault.secret_revealed`). The audit entry says whether a secret was set or cleared, never what it was.
- **Organization-scoped:** an item belongs to one organization and is seen only by those with access there.

## Permissions

| Permission | Lets you |
|---|---|
| `vault:read` | See items (never their secrets) and who revealed them. |
| `vault:reveal` | Reveal secrets, each one recorded. |
| `vault:manage` | Add, change and delete items and their secrets. |

Technicians have all three, and read-only users have `vault:read`. Owners and admins have everything. For a stricter setup, use a custom role without `vault:reveal`.

## Workflows

Each reveal raises `vault.secret_revealed` with the item's `name` and `kind`, the `reason`, and the `actorId`, never the secret. Use it, for example, to notify the account owner whenever a break-glass login is revealed.

## API

| Endpoint | |
|---|---|
| `GET /api/v1/vault/items?orgId=…&q=…` | Items you can see (never secrets) |
| `POST /api/v1/vault/items` | Add one (`orgId`, `name`, `kind`, `username`, `url`, `notes`, `tags`, `secret`) |
| `GET`, `PATCH`, `DELETE /api/v1/vault/items/{id}` | Read, change (send the `version` you read; `secret` replaces it, `clearSecret` removes it), delete |
| `POST /api/v1/vault/items/{id}/reveal` | `{ secret }`, with an optional `reason`; recorded and audited |
| `GET /api/v1/vault/items/{id}/reveals`, `GET /api/v1/vault/reveals` | Who revealed what, newest first (including deleted items) |
