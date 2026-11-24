# ADR-032: Huxley is the OAuth authorization server for its own MCP endpoint

- **Status:** Accepted (implementer's design, per the maintainer's "you choose")
- **Date:** 2026-11-25

## Context

MCP clients (Claude Code, Claude Desktop, IDE assistants) connected with an API token that people had to create and paste (ADR-030). The MCP authorization spec lets a client sign in through the browser instead. The server is an OAuth 2.1 resource server, and it names the authorization server that issues its tokens. Huxley already authenticates people (passwords, passkeys, SSO, two-factor), so it can issue those tokens itself.

## Decision

- **Huxley is the authorization server, for its own MCP endpoint only.** It isn't a general OAuth or OpenID provider.
- **Discovery:** `/mcp` answers a missing or bad token with `401` and a `WWW-Authenticate` header naming the protected resource metadata (RFC 9728) at `/.well-known/oauth-protected-resource/mcp`. That names the issuer, whose metadata (RFC 8414) is at `/.well-known/oauth-authorization-server`. Both need `auth.public_url`, and OAuth is off without it.
- **Clients register themselves** (RFC 7591, `/oauth/register`), and every client is a **public client**: no secret, whatever it asks for. MCP clients run on people's machines, where a secret can't be kept.
  - **Redirect URIs:** `https`, `http` on a loopback address (any port at authorization time, RFC 8252), or an app's own scheme. `javascript:`, `data:` and similar are refused.
  - **Abuse:** registration is open but rate-limited per address. Registrations that never lead to an approval are deleted after a day.
- **Authorization:** the web app's `/oauth/authorize` page asks the signed-in person.
  - The API checks the request first. An unknown client or unregistered redirect URI is shown as an error, never redirected to. Any other problem goes back to the client as an OAuth error with its `state`.
  - The page shows the client's self-chosen name, warns that it's self-chosen, and shows where the browser goes afterwards.
  - Approving needs a session, never a token. The page can't be framed.
- **PKCE with S256 is required.** Codes last five minutes and are single use. Exchanging a code twice revokes everything the first exchange issued.
- **Tokens:**
  - The approval is a **grant**: a person, a client and scopes. Scopes are permission keys, or `mcp` for everything the person's roles allow. They narrow the person's roles exactly as an API token's scopes do, never widen them.
  - **Access tokens** (`hxo_`) last an hour. **Refresh tokens** (`hxr_`) last 30 days and rotate on every use. Presenting a replaced refresh token revokes the grant, because it means the token was copied.
  - Like every Huxley secret, they're stored only as SHA-256 hashes.
- **Access tokens work only for MCP.** They're accepted on `/mcp` and on the API calls MCP tools make in-process, which the server marks and clients can't fake. The API itself refuses them. They're bound to the resource they were issued for (RFC 8707): the `resource` parameter must name this server's `/mcp`.
- **People see and end their grants** under **Profile → Connected apps**, and get a security notice when an app connects. Revocation (RFC 7009, `/oauth/revoke`) and a disabled account end grants immediately. Connecting, disconnecting and every revocation from reuse are in the audit log.

## Consequences

- Connecting a client is: add the URL, then approve in the browser. API tokens keep working for scripts, CI and the stdio bridge.
- The token endpoints are open to any origin (no cookies are involved), so browser-based clients work.
- **Not done yet:**
  - **Client ID metadata documents** (clients identified by an HTTPS URL): only registration is supported.
  - **Step-up scopes** (`403` with `insufficient_scope`): a refused tool call is reported as a tool error instead.
