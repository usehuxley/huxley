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

## Versions

Importing a newer spec (**Import as a new version**, or `PUT /api/v1/integrations/{id}/spec`) adds the next version. Every version keeps the spec as it was given and what was imported from it.

## Permissions

| Permission | Allows |
|---|---|
| `integration:read` | Viewing integrations and their actions (technicians and read-only users have it) |
| `integration:manage` | Importing specs and new versions |
