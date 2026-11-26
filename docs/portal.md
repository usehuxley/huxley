# Client portal

The **client portal** (the `portal` module) is a separate site where your clients' contacts sign in to see and raise their support requests. It runs as its own app (`portal`, at `HUXLEY_PORTAL_DOMAIN` in the compose stack) with its own sign-in, sessions and API, kept apart from staff accounts (ADR-040).

So far, contacts can be invited and can sign in. Tickets, the service catalog, help articles, devices and approvals follow, along with your branding and your clients' own sign-in providers.

## Setting it up

1. Turn on **Client portal** under **Settings → Modules**. The service desk preset has it on.
2. Make sure **email** works (`HUXLEY_EMAIL__SMTP_URL`), since sign-in links go by email.
3. Tell the API where the portal is with `HUXLEY_HTTP__PORTAL_PUBLIC_URL`, for example `https://support.example.com`. The compose stack sets it from `HUXLEY_PORTAL_DOMAIN`. Without it, links aren't emailed.

## Giving contacts access

- **Inviting:** on a contact's page, under **Portal access**, choose a **portal role** and press **Invite to the portal**. They get an email with a sign-in link. The contact needs an email address, and each address can have portal access only once.
- **Statuses:** a contact shows as **Invited** until they first sign in, then **Active**.
- **Other changes:** you can change their role, **email a new sign-in link**, or **disable** portal access. Disabling ends their sessions at once, and **Re-enable** gives access back.
- **Who can:** seeing who has access needs `crm:read` at the client. Inviting and changing access needs `crm:manage`.
- **Records:** changes are audited (`portal.account_invited`, `portal.account_updated`).

## Portal roles

| Role | What they can do |
|---|---|
| **Client user** | Their own tickets: see, raise, reply. The service catalog and help articles. |
| **Client approver** | As a client user, and approve what's sent to them. |
| **Client admin** | Every ticket of their company, its devices, and their colleagues' portal access. |

## Signing in

- **A sign-in link**, emailed when they enter their address. It works once, for 15 minutes. Opening the link shows a **Sign in** button rather than signing in straight away, because email security scanners open links too.
- **A password**, once they've set one in the portal. Five wrong passwords in a row lock the account for a minute. The wait doubles with each further miss, up to about an hour.
- **Not revealing accounts:** asking for a link answers the same whether or not the address has access, so nobody can find out who your clients are.
- **Sessions** last up to 14 days, ending after a day unused.
- **Separation:** the portal has its own session cookie (`hx_portal_session`) and its own API (`/api/portal/v1`). A portal session can't reach the staff API, and a staff session can't reach the portal's.

## API

- Staff:
  - `GET /api/v1/orgs/{id}/portal-accounts`
  - `POST /api/v1/contacts/{id}/portal-access` with `{ role }`
  - `PATCH /api/v1/portal-accounts/{id}` with `{ role, status, version }`
  - `POST /api/v1/portal-accounts/{id}/invitation` (a new sign-in link)
- The portal's own API, with portal sessions only:
  - `POST /api/portal/v1/auth/link`, `…/auth/redeem`, `…/auth/password`, `…/auth/logout`
  - `GET /api/portal/v1/me`
  - `PUT /api/portal/v1/me/password`
