# ImmyBot

The built-in **ImmyBot** pack (`huxley/immybot`) connects Huxley to your ImmyBot instance. It brings:

- **The `immybot` integration**, with these actions:
  - `listTenants`
  - `listComputers` (pages with `skip` and `take`; `filter`, `tenantId`)
  - `getComputer`, `isComputerOnline`, `reinventoryComputer`
  - `runMaintenance`: start sessions on computers or whole tenants
  - `getSession`, `getSessionLogs`, `cancelSession`
  - `latestActionsForComputer`
- **The *Run ImmyBot maintenance on a computer* workflow.**
  - It starts a session on a computer: a full run of what's deployed to it, or just detection or inventory, optionally with Windows updates.
  - It waits up to 30 minutes and, given a ticket, adds the outcome and the session's last log lines to it as an internal note.
  - Its outputs are `session_id`, `status` and `passed`.
  - Reboots are suppressed unless you choose otherwise. If the computer already has a session running, it follows that one.
- **The *Sync ImmyBot computers* workflow and the *ImmyBot computers* dataset.** Every hour, the workflow reads every computer into the dataset and runs your asset mappings on it (see [Computers as assets](#computers-as-assets)).

The integration covers the operations the pack uses. ImmyBot's full API, over 500 operations, is described at `https://<instance>.immy.bot/swagger`. Import that spec as a separate integration if you need the rest; the connection details below work for it too.

## Setting it up

1. **Register an app** in your Entra admin center, with default settings, and add a client secret. Note its client (application) ID and the secret's value.
2. **Let it into ImmyBot.**
   - In Entra, under **Enterprise applications**, open the app and copy its **object ID**. This is not the application ID.
   - In ImmyBot, under **People**, add a person with that object ID as their **AD External ID**.
   - Create a user from that person and give it admin rights.
3. **Install the pack** under **Settings → Packs**, and publish the workflows.
4. **Make a connection** on the integration's page with the key **`immybot`** (the workflows use it) and the scheme **entra**:
   - **instance**: your ImmyBot subdomain, `acme` for `acme.immy.bot`
   - **tenant**: your Entra tenant's ID or primary domain
   - **Client ID** and **Client secret**: the app's
   - **Scopes**: leave empty. Huxley asks for `https://<instance>.immy.bot/.default`.

Huxley gets a token from Entra when it needs one and reuses it until it nearly expires. ImmyBot allows 120 requests a minute per credential.

## Computers as assets

1. **Publish *Sync ImmyBot computers*** (at your MSP), and run it once to fill the **ImmyBot computers** dataset. Its columns are:
   - `id`, `name`, `tenant_id`, `tenant_name`
   - `online`, `onboarding`, `os`, `manufacturer`, `model`, `serial`
   - `domain`, `ip`, `last_user`, `primary_user_email`
   - `pending_reboot`, `os_end_of_life`
2. **Tell Huxley which client is which.** On each client, add an organization variable `immybot_tenant_id` holding the client's tenant ID in ImmyBot.
3. **Add a mapping** under **Settings → Asset sync**:
   - **Dataset:** *ImmyBot computers*, with source `immybot`.
   - **Match keys:** match by *Id in the source* from `id`, then *Serial number* from `serial`.
   - **Fields:** fill in the name, serial number and manufacturer from their columns.
   - **Each asset belongs to:** *the organization whose variable matches a column*, with column `tenant_id` and variable `immybot_tenant_id`.
4. Check it with **Preview**. Each hourly sync then updates the assets too.

If you use both ImmyBot and ConnectWise Automate, map one of them as the source of new assets. Give the other the same match keys, with **Create assets** off, so each computer stays one asset.

## Running maintenance from a ticket

Make *Run ImmyBot maintenance on a computer* an [Action](forms.md) on tickets, or start it from another workflow. **Detection** reports what maintenance would change without changing anything, which is useful before a full run.

## Good to know

- **Where the API details come from.** The integration follows the OpenAPI spec published by an ImmyBot instance (API v1). It's tested against a mock server, not a live instance.
- **Tokens:** the app needs no Microsoft Graph permissions for this. ImmyBot accepts the token because the app is an ImmyBot user.
