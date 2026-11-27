# Provisioning (SCIM)

Your identity provider can manage Huxley accounts for you over SCIM 2.0: people are created when they're assigned the Huxley app, updated when they change, and disabled the moment they're unassigned or leave. Groups you push become Huxley teams, and the identity provider manages their members.

Provisioning belongs to a sign-in provider (Settings → Single sign-on). People it creates sign in through that provider and have no Huxley password.

## Turning it on

1. Open **Settings → Single sign-on**, then the provider.
2. Under **Provisioning (SCIM)**, choose **Turn on provisioning**.
3. Copy the **Tenant URL** and the **secret token**. The token is shown once. Making a new one replaces it, and the old one stops working straight away.

The tenant URL is your Huxley address followed by `/scim/v2`, for example `https://huxley.example.com/scim/v2`. It's built from `auth.public_url`. If you run your own reverse proxy, send `/scim/*` to the API, as you do for `/api/*` (the bundled Caddyfile already does).

### Microsoft Entra ID

1. In the Huxley enterprise application, open **Provisioning**, then **Get started**, and set **Provisioning Mode** to **Automatic**.
2. **Tenant URL:** the tenant URL. **Secret Token:** the token. Choose **Test Connection**.
3. Under **Mappings**, keep the user attributes `userName`, `active`, `displayName`, `emails[type eq "work"].value`, `name.givenName`, `name.familyName` and `externalId`. Remove any other attributes. Huxley ignores attributes it doesn't store, but fewer means quicker cycles.
4. Assign users and groups to the application, then turn **Provisioning Status** on.

### Okta

1. In the Huxley app integration, under **General**, set provisioning to **SCIM**.
2. Under **Provisioning → Integration**:
   - **SCIM connector base URL:** the tenant URL.
   - **Unique identifier field for users:** `userName`.
   - **Supported provisioning actions:** Push New Users, Push Profile Updates and Push Groups.
   - **Authentication Mode:** HTTP Header, with the token as the bearer token.
3. Under **Provisioning → To App**, enable Create Users, Update User Attributes and Deactivate Users.

### Other providers

JumpCloud, OneLogin, Authentik, Keycloak (with a SCIM plugin) and others work the same way: a base URL and a bearer token. Huxley supports the parts of SCIM 2.0 that identity providers use:

- Users and Groups.
- `eq` filters on `userName`, `externalId` and `emails.value` (users), and on `displayName` and `externalId` (groups).
- Paging, PUT, and PATCH, including Entra's and Okta's styles.
- The discovery endpoints.

## What happens in Huxley

| In the identity provider | In Huxley |
|---|---|
| A user is assigned | A user is created, with their email verified by the provider. If someone with that email already exists, they're linked instead: their roles and history are kept. |
| Their name or email changes | It changes in Huxley. |
| They're deactivated or unassigned | They're **disabled** and every session ends at once. Reactivating them restores access. |
| They're deleted | They're disabled, removed from the provider's teams, and unlinked. The user and everything they did stay, for the audit log. |
| A group is pushed | A team is created, or an existing team with that name that no other provider manages is adopted. |
| Members change | The team's members change to match. The provider is the source of truth: people added to the team by hand in Huxley are removed at its next sync. |
| A group is deleted | The team is removed, and with it the roles it held. |

**Roles stay in Huxley.** Give roles to provisioned teams in Settings → Teams. Their members get the roles as the identity provider adds them. You can also map sign-in claims to roles on the provider (role mappings); those apply when someone signs in.

Everything provisioning does is in the audit log (`user.provisioned`, `user.deprovisioned`, `team.provisioned` and the usual user and team records). The actor is the system, by way of the API.

## Turning it off

**Turn off** on the provider's Provisioning card removes the token. What was provisioned stays as it is, and you manage it by hand from then on.
