# ConnectWise Automate

The built-in **ConnectWise Automate** pack (`huxley/cwautomate`) connects Huxley to your Automate server. It brings:

- **The `cwautomate` integration**, with these actions:
  - `listComputers`: a page of computers, with `page`, `pagesize` (up to 1000), `condition` (e.g. `Type = 'Server'` or `(Client.Name like '%Acme%')`) and `orderby`.
  - `runCommand`: queue a command on a computer.
  - `getCommandResults`: read commands' status and output.
- **The *Run a command on an Automate computer* workflow.** It runs a PowerShell (or cmd) command line on a computer, waits up to two minutes, and, given a ticket, adds the output to it as an internal note. Its outputs are the command's `status` and `output`.

## Setting it up

1. **An integrator login.** In Automate, under **System → Integrator Logins**, make a login for Huxley with only the rights it needs.
2. **A ClientID** from the ConnectWise developer portal ([developer.connectwise.com/ClientID](https://developer.connectwise.com/ClientID)). Automate 2020 Patch 11 and later require one on every API call.
3. **Install the pack** under **Settings → Packs**, and publish the workflow.
4. **Make a connection** on the integration's page with the key **`automate`** (the workflow uses that key) and the scheme **integrator**:
   - **server**: your Automate host, e.g. `automate.example.com`
   - **Username** and **Password**: the integrator login
   - **Client ID**: your ClientID

Huxley logs in with the integrator login when it needs a token, reuses the token (about an hour) until it nearly expires, and sends the ClientID with every call.

## Running a command

Start *Run a command on an Automate computer* with the computer's Automate ID, the command, whether to use PowerShell (the default) or cmd, and optionally a ticket. You can also make it an [Action](forms.md) on tickets, so technicians can run a diagnostic from the ticket and keep the output there. Commands run in `%WINDIR%\Temp` through Automate's shell command (command 2), as `powershell.exe -NonInteractive -Command …` or `cmd.exe /c …`. If a command takes longer than two minutes, the run fails saying so, but Automate may still finish it.

## Good to know

- **How it was built.** Automate's API reference is behind ConnectWise's developer portal, so this integration follows the requests made by the widely used open-source AutomateAPI PowerShell module: `POST /cwa/api/v1/apitoken`, `GET /computers`, and `POST`/`GET /Computers/{id}/CommandExecute`. It's tested against a mock server, not a live Automate. If your server answers differently, [update the integration](integrations.md#updating) with your edits, and tell us so it can be fixed for everyone.
- **Devices as assets.** Coming next: a workflow that keeps an Automate devices dataset up to date, which [asset sync](assets.md#syncing-from-datasets) turns into assets of the right clients.
