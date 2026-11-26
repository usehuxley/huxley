# Client portal

The **client portal** (the `portal` module) is a separate site where your clients' contacts sign in to see and raise their support requests. It runs as its own app (`portal`, at `HUXLEY_PORTAL_DOMAIN` in the compose stack) with its own sign-in, sessions and API, kept apart from staff accounts (ADR-040).

Contacts can sign in, see their tickets, raise new ones and reply, in your branding. The service catalog, help articles, devices and approvals follow, along with your clients' own sign-in providers.

## Setting it up

1. Turn on **Client portal** under **Settings → Modules**. The service desk preset has it on.
2. Make sure **email** works (`HUXLEY_EMAIL__SMTP_URL`), since sign-in links go by email.
3. Tell the API where the portal is with `HUXLEY_HTTP__PORTAL_PUBLIC_URL`, for example `https://support.example.com`. The compose stack sets it from `HUXLEY_PORTAL_DOMAIN`. Without it, links aren't emailed.

## Branding and domains

The portal wears your brand. Set it under **Settings → Organizations → (your MSP) → Settings → Branding**:

- **Brand name:** shown on the sign-in page, in the sidebar and in page titles. It defaults to the organization's name.
- **Accent color:** buttons, links and highlights. Text on it switches between white and near-black to stay readable.
- **Logo:** a PNG, JPEG, WebP or SVG up to 200 KB, shown on the sign-in page and in the sidebar.
- **Support email:** where contacts are told to write.

Like every setting, these apply to the organization and everything below it, so **a client can have its own branding** by setting any of them on the client. Lock a setting at the MSP to stop clients changing it.

**Which branding shows:**
- Before signing in, the portal shows the branding of the organization whose **portal domain** matches the address it's opened at. Without a match, it shows your MSP's.
- After signing in, it shows the branding of the contact's company, with the client's own settings over the MSP's.

**Portal domain:**
- Each organization can have its own portal address, such as `support.contoso.com`, and a domain belongs to one organization only.
- Sign-in links for its contacts, and those of organizations below it, use that address. Without one, they use `HUXLEY_HTTP__PORTAL_PUBLIC_URL`.
- Point the domain's DNS at your server. The compose stack's Caddy serves `HUXLEY_PORTAL_DOMAIN` with its own certificate, so add a site block for each extra domain, reverse-proxied to `portal:3000` like the first.

## Giving contacts access

- **Inviting:** on a contact's page, under **Portal access**, choose a **portal role** and press **Invite to the portal**. They get an email with a sign-in link. The contact needs an email address, and each address can have portal access only once.
- **Statuses:** a contact shows as **Invited** until they first sign in, then **Active**.
- **Other changes:** you can change their role, **email a new sign-in link**, or **disable** portal access. Disabling ends their sessions at once, and **Re-enable** gives access back.
- **Who can:** seeing who has access needs `crm:read` at the client. Inviting and changing access needs `crm:manage`.
- **Records:** changes are audited (`portal.account_invited`, `portal.account_updated`).

## People

Client admins manage their own organization's portal access under **People**: no ticket to the service desk needed.

- **The list** shows their company's contacts, and who can sign in (invited, has access, or no longer has access).
- **Add someone** creates a contact at their company and, if they ask, gives them access with a role. An invitation is emailed.
- **Give access** to a colleague who's already a contact, or **change** what they can do. **Remove access** signs them out everywhere. **Restore access** brings it back.
- **Limits:** only their own company's contacts, and never their own access (another admin, or the service desk, changes that). Staff see and change the same accounts on the contact's page.

## Portal roles

| Role | What they can do |
|---|---|
| **Client user** | Their own tickets: see, raise, reply. The service catalog and help articles. |
| **Client approver** | As a client user, and approve what's sent to them. |
| **Client admin** | Every ticket of their company, its devices, and their colleagues' portal access. |

## Tickets

- **Seeing tickets:** contacts see the tickets they asked for. **Client admins** can switch to **all of their company's** tickets.
- **The conversation:** a ticket shows its status, its details, and the public conversation: your public notes and the resolution. **Internal notes, assignees, time, queues and SLA details never reach the portal.**
- **Raising a ticket:** a ticket raised in the portal belongs to the contact's company, with the contact as requester and its source set to `portal`. It gets the default type, routing and SLA, and whatever workflows act on new tickets.
- **Replying:** a reply is a public note by the contact. Replying to a resolved ticket reopens it. Closed tickets can't be replied to, so the contact raises a new one.
- **Notifications:** with the **Service desk notifications** pack (1.1.0), whoever a ticket is assigned to is told when the requester replies in the portal, as they are for email replies.
- **The service desk module** must be on for the company.

## Signing in

- **A sign-in link**, emailed when they enter their address. It works once, for 15 minutes. Opening the link shows a **Sign in** button rather than signing in straight away, because email security scanners open links too.
- **Their organization's account**, when the client has a sign-in provider (below). They enter their email, and the portal sends them to their company's sign-in page and back.
- **A password**, once they've set one in the portal. Five wrong passwords in a row lock the account for a minute. The wait doubles with each further miss, up to about an hour.
- **Not revealing accounts:** asking for a link answers the same whether or not the address has access, so nobody can find out who your clients are.
- **Sessions** last up to 14 days, ending after a day unused.
- **Separation:** the portal has its own session cookie (`hx_portal_session`) and its own API (`/api/portal/v1`). A portal session can't reach the staff API, and a staff session can't reach the portal's.

## Requests

**Request something** in the portal lists the forms you offer there. Contacts fill one in, and its workflows run, the same as when staff fill it in.

- **Offering a form:** in the form's editor, tick **Offer in the client portal**. Optionally give a **Portal category**, such as *Accounts* or *Hardware*, to group forms. A form is offered to contacts of its organization and of the organizations below it, so an MSP's form reaches every client.
- **What the workflows get:** the answers as inputs, and `trigger.context` with `client` (the contact's company), `contact` (name, email, phone…) and `portal` (their portal account and role). The runs happen for the contact's company.
- **Who the runs act as:** each workflow's run-as service account, as for a public link. They never act as a person.
- **No lookups:** forms that look choices up through an integration can't be offered in the portal.
- **Following up:** contacts see **Your requests** with a status: *Received* (no workflow to run), *In progress*, *Done*, or *Being looked at* (a run failed or was cancelled; check the run history).

## Help articles

**Help articles** in the portal shows knowledge base articles meant for clients: published, with the visibility **Clients** or **Public**. Contacts see those of their company and of the organizations above it, so an MSP's articles reach every client, and a client's own documentation reaches only that client.

- **Only the published revision** shows. Drafts, reviews and history stay with staff.
- **Search** matches titles and text, best match first, and categories with articles are listed for browsing.
- **The knowledge module** must be on for the company. The portal's navigation shows only the parts that are on (tickets, help articles).

## Devices

**Devices** in the portal lists the company's assets in use (not retired or disposed), from the CMDB.

- **Client admins** see all of their company's devices. **Everyone else** sees the devices assigned to them.
- **Shown:** name, type, serial number, hostname, make and model, site, who it's assigned to, and the warranty end date. Notes, custom fields, outside ids and sync details stay with staff.
- **Raise a ticket about it:** from a device's page. The new ticket is linked to the device, so staff see it on the ticket's Assets card.
- **The CMDB module** must be on for the company.

## Approvals

Workflow **approval** steps can ask client contacts, who decide in the portal's **Approvals** page.

- **In the step's `approvers`:**
  - `contact:<id>` names a contact. They need portal access at the run's company, as a client approver or client admin.
  - `client_approvers` means all of the run's company's client approvers and client admins.
  - Mix them with staff (`user:`, `team:`, `role:`) freely. The first decision counts.
- **Telling them:** they're emailed a link to the approval in the portal, at their company's portal domain or `HUXLEY_HTTP__PORTAL_PUBLIC_URL`, when email is set up. The portal's home page shows what's waiting.
- **The decision** is the step's output, as for staff. `by` has the contact's `id`, `name` and `email`, plus `contact` and `portal_account`.
- **The run's company** decides who `client_approvers` is, so run the workflow for the client: a form submitted for it, a trigger at it, or a workflow saved there.

## Reports

**Reports** in the portal shows the dashboards you publish there, to client admins, with their own company's numbers.

- **Publishing:** in a shared dashboard's editor, tick **Show it in the client portal**. A personal dashboard can be shared first (needs `dashboard:manage`). It shows in the portal of its organization and the organizations below it, so a dashboard at the MSP reaches every client.
- **Their numbers only:** every widget is narrowed to the contact's company. Only tickets, time entries and assets show there, and only where those modules are on for the company. Other widgets show as unavailable.
- **Range:** they can pick a range. The dashboard's own range is the default.

## Two-factor sign-in

Contacts can add a code from an authenticator app (Microsoft Authenticator, Google Authenticator, 1Password and others) to signing in with a link or password: **Profile**, **Manage two-factor sign-in**.

- **Recovery codes:** ten, shown once when they turn it on. Each signs in once without the app. They can replace them from the same page.
- **Requiring it:** turn on **Require two-factor sign-in** in the organization's settings (Portal group). It's inherited by the organizations below and can be locked. Someone without an authenticator can then only reach the setup page after signing in, until they've set one up.
- **Signing in with their organization's account** isn't asked for a code: their organization's sign-in decides that.
- **A lost phone:** on the contact's page, **Reset two-factor sign-in**. Their sessions end, and they set it up again if it's required.
- **Wrong codes** count toward the same lockout as wrong passwords. A sign-in allows five tries at the code within five minutes.
- **Turning it off** needs a current code, and isn't possible while their organization requires it.

## Signing in with their organization

Contacts can sign in with their company's own accounts, such as the client's Microsoft Entra tenant. Add it on the client's page, **Portal** tab, **Add a provider** (needs `crm:manage`).

- **Microsoft Entra ID:** register an app in the client's tenant with the redirect URI `https://<portal address>/auth/sso/callback` (the client's portal domain, or `HUXLEY_HTTP__PORTAL_PUBLIC_URL`), and create a client secret. Enter the tenant ID, client ID and secret. Any other OpenID Connect provider works with its issuer URL.
- **Email domains:** the addresses it signs in, e.g. `contoso.com`. A domain belongs to one provider. When someone enters an address at that domain, the portal goes to their organization's sign-in page instead of emailing a link.
- **Who gets in:** contacts who already have portal access, at the client or an organization below it. Turn on **Give portal access to contacts with a matching address** to give contacts an account, with the role you choose, the first time they sign in. Nobody without a contact record gets in.
- **The link to their account** is kept after the first sign-in, so a later change of address doesn't lock them out.
- **The client secret** is encrypted with the client's key and is never shown again.
- **Safety:** the sign-in is bound to the browser that started it (a cookie), works once within 10 minutes, and only ever returns to this portal's callback page. ID tokens are verified (signature, issuer, audience, expiry, nonce), with PKCE.

## API

- Staff:
  - `GET /api/v1/orgs/{id}/portal-accounts`
  - `POST /api/v1/contacts/{id}/portal-access` with `{ role }`
  - `GET`/`POST /api/v1/orgs/{id}/portal-identity-providers`; `PUT`/`DELETE /api/v1/portal-identity-providers/{id}`
  - `POST /api/v1/portal-accounts/{id}/mfa-reset`
  - `PATCH /api/v1/portal-accounts/{id}` with `{ role, status, version }`
  - `POST /api/v1/portal-accounts/{id}/invitation` (a new sign-in link)
- The portal's own API, with portal sessions only:
  - `POST /api/portal/v1/auth/link`, `…/auth/redeem`, `…/auth/password`, `…/auth/logout`
  - `POST /api/portal/v1/auth/sso/start` with `{ email, redirect }` (404 when no provider has the domain); `POST …/auth/sso/callback` with `{ state, code }`
  - `GET /api/portal/v1/branding?host=` (before signing in; also in `/me` after)
  - `GET /api/portal/v1/me`
  - `GET`, `POST /api/portal/v1/tickets` (`status=open|closed`, `scope=mine|company`, `q`); `GET …/tickets/{id}`; `POST …/tickets/{id}/replies`
  - `PUT /api/portal/v1/me/password`
  - `GET /api/portal/v1/articles` (`q`, `category`), `GET …/articles/{id}`
  - `GET`/`POST /api/portal/v1/people`; `POST`/`PATCH …/people/{contact}/access` (client admins)
  - `GET /api/portal/v1/dashboards`, `GET …/dashboards/{id}?range=` (client admins)
  - `GET /api/portal/v1/approvals`, `GET …/approvals/{id}`, `POST …/approvals/{id}/decision` `{ approve, comment }`
  - `GET /api/portal/v1/devices` (`q`), `GET …/devices/{id}`; `POST …/tickets` takes an optional `assetId`
  - `GET /api/portal/v1/catalog`, `GET …/catalog/{id}`, `POST …/catalog/{id}/requests` `{ answers }`; `GET …/requests`
  - Two-factor: `…/auth/link` redemption and `…/auth/password` answer `202 { challenge }` when a code is needed; finish with `POST …/auth/mfa` `{ challenge, code }`. `GET …/me/mfa`; `POST …/me/mfa/totp` (start), `…/totp/confirm` `{ code }` (returns recovery codes), `…/totp/disable` `{ code }`, `…/recovery-codes`
