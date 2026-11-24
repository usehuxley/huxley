# Integrations

An integration is an API Huxley knows how to call, imported from the vendor's spec (design doc §7.1, ADR-023). Import one under **Integrations**, or with `POST /api/v1/integrations` (`{ "orgId", "key"?, "spec" }`).

## What gets imported

- **Formats:** OpenAPI 3.0 and 3.1, and Swagger 2.0, as JSON or YAML. The spec can be up to 20 MB with up to 5,000 operations.
- **Servers:** base URLs, with any `{variables}` such as a tenant or region. Connections fill these in.
- **Authentication:** API keys (header, query or cookie), HTTP basic, bearer tokens, OAuth 2.0 authorization code and client credentials, and OpenID Connect.
- **Actions,** one per operation:
  - **Key:** the operation id, or `<method>_<path>` when there is none.
  - **Name and group:** from the summary and the first tag.
  - **Inputs:** one schema. Each parameter appears by name and the request body as `body`. `Authorization`, `Content-Type` and `Accept` headers are left out; the connection and the body type supply them.
  - **Output:** the schema of the first successful JSON response.

## Import notes

Published specs often have small mistakes. When the importer can work around one, it does, and lists what it did with a pointer into the spec:

- a repeated operation id gets a numbered key
- a path placeholder that isn't declared is added as a text input
- a dangling `$ref`, an external `$ref`, or an unsupported auth type (OAuth 2.0 implicit or password, HTTP digest) is skipped

Only a file that isn't JSON or YAML, isn't OpenAPI or Swagger, or is over the limits is refused.

## Your edits (overlays)

On an integration's page, **Edit** on an action changes how it's shown: its name, group and description. You can also **hide** it from the action picker. These edits form the integration's **overlay**. It's stored apart from the vendor's spec, so updating to a newer spec keeps it, and an edit follows its action when the vendor renames it. Workflows still call actions by key, so nothing you change here breaks them, and hidden actions keep working where they're already used. The API is `PUT /api/v1/integrations/{id}/overlay`, which needs `integration:manage`. Pagination and rate-limit settings will come later.

## Built-in integrations

**Built-in integrations** on the Integrations page are curated specs shipped with Huxley. **Add** puts one in the chosen organization under its own key. From then on it's an ordinary integration: you can update it, pin it, and give it connections.

- **Microsoft Graph** (`msgraph`), for Microsoft 365 and Entra ID:
  - users: list, get, create, update, delete, revoke sign-in sessions, list group memberships
  - licences: list SKUs, assign and remove
  - groups: list, list members, add and remove members
  - mail: send mail

  **To connect it:**
  1. Register an app in the tenant's Entra admin center.
  2. Add a client secret.
  3. Grant application permissions such as `User.ReadWrite.All`, `Group.ReadWrite.All`, `Organization.Read.All` and `Mail.Send`, with admin consent.
  4. Make a connection with the scheme **app**: the client ID, the secret, and the **tenant** server variable (the tenant ID or `contoso.onmicrosoft.com`).

  **For many client tenants (GDAP):** make **one shared connection at the MSP** with the partner's multi-tenant app, and set its tenant to `{{ vars.m365_tenant }}`. Then give each client organization an `m365_tenant` variable. A run in a client organization gets a token for that client's tenant, and each tenant's token is cached separately (ADR-027). This works for client-credentials connections only.
- **Slack** (`slack`): post a message and list channels, with a bot token (`xoxb-…`) that has `chat:write`. Slack answers 200 even when a call fails, so check `ok` in the output.

## Updating

Vendors change their APIs. To bring an integration up to date, import the newer spec under **Import a newer spec**. You get a review first, and nothing changes until you apply it.

- **Breaking changes** are flagged. Workflows or connections using the integration could stop working:
  - an action removed
  - a changed method or path
  - an input removed, made required, or given a different type
  - a changed request body type
  - an auth scheme removed or changed
- **Workflows this would break:** the published workflows, among those you can see, whose steps call a removed or broken action, with each affected step and its problem. Each workflow links to its page.
- **Other changes** are listed as notes: new actions, new optional inputs, a different response shape (check expressions that read it), different accepted auth, a deprecation, or new servers.
- **Renames:** an action renamed by the vendor (same method and path, new operation id) is shown as a rename, and it isn't breaking. Applying the update records an alias, so steps using the old key keep working, including after later renames.

Applying adds the next version and makes it current. If someone else changed the integration after your review, applying is refused, so review again. The API equivalent is `POST /api/v1/integrations/{id}/preview` (`{ "spec" }`), then `PUT /api/v1/integrations/{id}/spec` (`{ "spec", "baseVersion" }`).

Every version keeps the spec as it was given and what was imported from it. **Make current** on an earlier version rolls back (`PUT /api/v1/integrations/{id}/current`, `{ "version" }`). The next import still gets a new version number.

## Connections

A connection holds credentials for one of an integration's auth schemes. Create connections on the integration's page, or with `POST /api/v1/connections` (`{ "orgId", "integrationId", "connection": { "name", "authScheme", "settings", "secrets", "baseUrl", "serverVariables", "shared" } }`).

**Where a connection lives:** it belongs to an organization, and its integration must belong to that organization or one above it. For example, an MSP imports an API once, and each client gets a connection with its own credentials. Mark a connection **shared** to let organizations below its owner use it as well.

**What each auth scheme asks for:**

| Auth scheme | Settings | Secrets |
|---|---|---|
| API key | | `key` |
| HTTP basic | `username` | `password` |
| Bearer token | | `token` |
| OAuth 2.0, OpenID Connect | `client_id`, `scopes` | `client_secret` |

**How secrets are handled:**
- Secrets are encrypted with the organization's data key and never shown again. The API and the page only say which ones are set.
- When you change a connection, secrets you leave out keep their stored value. Switching to a different auth scheme clears them all.
- The audit log records which secrets changed, never their values.

**Base URL:** a connection uses the spec's first server URL, with its `{variables}` filled from the connection's server variables. If the spec has no servers, or you want another region or a sandbox, give a base URL to override it. Auth URLs can use server variables too, such as a tenant in `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token`. There, values may only contain letters, digits, dots, hyphens and underscores, so they can't change where the URL points.

**OAuth 2.0:**

- **Client credentials:** the connection fetches a token when one is needed, and reuses it until a minute before it expires.
- **Authorization code:** the connection starts as **Needs sign-in**. Someone with `connection:manage` presses **Sign in**, signs in at the provider, and is sent back to `<auth.public_url>/integrations/oauth/callback`. That URL is the one to register with the provider, so `auth.public_url` must be set.
  - The sign-in uses PKCE (S256) and a single-use `state` that only the person who started it can complete, within ten minutes.
  - Tokens are encrypted, kept apart from the credentials you entered, and refreshed before they expire. A rotated refresh token is kept.
  - If the provider revokes the sign-in, the connection goes back to **Needs sign-in**, with the reason shown.
- **Both:** a connection fetches or refreshes one token at a time, so concurrent workflow steps don't compete for it. Token requests follow the same outbound rules as HTTP steps (`HUXLEY_ENGINE__EGRESS__*`).

## Calling actions

Workflows call an integration's actions with the `integration.action` step:

```yaml
- id: open
  type: integration.action
  action: tickets/listTickets         # <integration key>/<action key>
  with: { status: open }              # parameters by name; the request body as `body`
- id: create
  type: integration.action
  action: tickets/createTicket
  connection: contoso                 # optional
  with:
    body: { subject: "{{ inputs.subject }}" }
```

- **Integration:** found by key in the run's organization or the nearest organization above it that has one.
- **Version:** a run uses the integration version that was current when it started, so an update mid-run can't change what it calls. Any integration it couldn't see at the start is pinned when the run first calls it.
- **Renamed actions:** an action the vendor renamed (same method and path) is still found under its old key. Updates carry these aliases forward.
- **Connection:** the one named, or else the nearest connection to the integration: the run's organization's own first, then **shared** ones from organizations above. If none fits, the step fails with `kind: connection`.
- **Inputs:** checked against the action's input schema before anything is sent. A mismatch fails with `kind: inputs` and lists the problems.
- **The request:** built from the action's method, path and parameter locations, and authenticated from the connection (including OAuth 2.0 tokens). It's sent like an `http.request` step, so the same outbound rules, response size limit, error kinds and retries apply. The step's output is the response body.
- **Supported bodies:** JSON, form-encoded and text. Multipart bodies aren't supported yet.

## Permissions

| Permission | Allows |
|---|---|
| `integration:read` | Viewing integrations and their actions (technicians and read-only users have it) |
| `integration:manage` | Importing specs and new versions |
| `connection:read` | Viewing connections, never their secrets (technicians have it) |
| `connection:manage` | Creating connections and changing their credentials |
