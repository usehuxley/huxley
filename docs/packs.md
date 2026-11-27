# Packs

A **pack** bundles integrations, workflows, forms, scripts, templates and datasets so they can be shared between instances, by Huxley, MSPs and the community (design doc §6.6, ADR-028). This page covers installing, exporting and signing packs, the built-in packs, and the format.

## Installing a pack

In **Settings → Packs**, choose the organization, then upload a `.hxpack`. Before anything changes, you see the review:

- what it adds, and for each item whether it **adds** it, **updates** something this pack installed before, or **conflicts** with something else of the same key
- **where it connects**: the hosts of its integrations and of literal URLs in its HTTP steps
- the **scripts** it adds, which run in sandboxes on your runners
- **who signed it**, with a warning if it's unsigned or signed by a key this instance doesn't trust

Things to know about installing:

- **Workflows arrive as drafts.** Nothing is published until someone reviews it.
- **Where they go:** a built-in pack's workflows go into the organization's System folder, and another pack's into a folder named after the pack (see [Folders](workflows.md#folders)).
- **Packs never overwrite** what they didn't install. A conflict blocks the install until you rename or remove the other item.
- **Upgrading** (uploading a newer version) updates what the pack installed, in place:
  - scripts get a new version and workflows a new draft
  - integration specs are updated, and your own overlay edits stay
  - the pack's templates are replaced
  - dataset definitions are changed to match (new columns added); their rows are kept
  - items an older version had and the new one doesn't are left alone
- **Permissions:** installing needs `pack:install` *and* the permissions for what the pack adds (`integration:manage`, `form:manage`, `script:manage`, `dataset:manage`, `workflow:write`). A pack never lets anyone do more than they could by hand.
- Installs are audited (`pack.installed`). Uploads are kept for a day, for reviewing.

To trust your own or a partner's signing key, list it in the server config:

```toml
[packs]
trusted_keys = ["hxpk_…"]
```

Or set `HUXLEY_PACKS__TRUSTED_KEYS='["hxpk_…"]'` in the environment.

The API is `POST /api/v1/packs/uploads?orgId=` (the raw file; it returns the review), `POST /api/v1/packs/uploads/{id}/install`, and `GET /api/v1/packs?orgId=`.

## From a registry

A **pack registry** is a folder of packs on a web server, with an index listing them (ADR-093). An MSP might host one for its own packs, and a community registry can host shared ones. Configure the registries an instance browses:

```toml
[[packs.registries]]
url = "https://packs.contoso.example/index.json"
key = "hxpk_…"   # optional: the registry's key; then its index must be signed by it
```

Or `HUXLEY_PACKS__REGISTRIES='[{url="https://packs.contoso.example/index.json", key="hxpk_…"}]'`.

Each registry gets a card in **Settings → Packs** listing its packs. For each one it shows:

- the versions it offers, newest first;
- who published it, and whether it's signed by a key this instance trusts;
- what's installed in the organization, and when a newer version is available.

**Review** downloads the version you pick and shows the same review as an upload, so everything above about installing applies. A registry adds two checks of its own:

- **The download must match the index.** The index lists every pack's SHA-256, and a file that doesn't match is refused. So is a file whose manifest names a different pack or version.
- **The index can be signed.** With `key` set, Huxley fetches `index.json.sig` and refuses the index unless the registry's key signed it, so nobody can swap packs or hashes in transit or on the server. Packs keep their own signatures too. Trust (`packs.trusted_keys`) is about who wrote a pack; the registry key is about where its list came from.

Indexes are cached for five minutes. The API is `GET /api/v1/packs/registry?orgId=` and `POST /api/v1/packs/registry/review?orgId=`, with `{ registry, key, version }`.

### Hosting a registry

Put your signed `.hxpack` files in a folder and index it:

```sh
huxley pack index ./packs --base-url https://packs.contoso.example --name "Contoso packs" --key registry.key
```

That checks each pack as an install would, then writes:

- `index.json`: every version of every pack, with its SHA-256, size, signer and contents;
- `index.json.sig`: the index's signature, written only with `--key`.

Serve the folder over HTTPS from anything that serves static files: object storage, GitHub Pages, nginx. Re-run the command when you add a version, and keep old versions in the folder so instances can still review them.

## Built-in packs

Huxley's built-in integrations (Microsoft Graph and Slack) are packs too: `huxley/msgraph` and `huxley/slack`. So are `huxley/servicedesk-email`, the service desk's *Poll mailboxes*, *Email to ticket* and *Email replies* workflows, `huxley/servicedesk-dispatch`, its *Dispatch tickets* workflow, `huxley/servicedesk-notifications`, which tells technicians about assignments, replies and SLAs, `huxley/servicedesk-chat`, which posts urgent tickets and missed SLAs to Slack or Teams, `huxley/servicedesk-search`, which embeds tickets for searching by meaning, and `huxley/knowledge-search`, which does the same for published articles, `huxley/cwautomate`, for running commands on ConnectWise Automate computers (see [ConnectWise Automate](cwautomate.md)), `huxley/immybot`, for ImmyBot maintenance and computers (see [ImmyBot](immybot.md)), and `huxley/timesheet-reminders` (see [Timesheets](workforce.md#timesheets)). They ship inside the binary, so they're trusted by construction, and the review labels them **Built in**.

You can add one from the Integrations catalog, or review it under **Settings → Packs → Built-in packs**. Either way it's recorded as an installed pack, so when a newer Huxley ships a newer version, it arrives as an upgrade you review.

## Exporting a pack

**Settings → Packs → Export a pack** bundles the organization's integrations (with their overlays), workflows (their drafts), forms and scripts into an unsigned `.hxpack`. Connections, credentials and secrets are never included. Exporting needs `pack:install`, plus read access to what you export.

To sign it, use your key:

```sh
huxley pack sign onboarding-1.0.0.hxpack --key contoso-packs.key
```

The API is `POST /api/v1/packs/export`.

## The format

A `.hxpack` file is a zip:

```text
manifest.json              what it is, what it adds, and every file's SHA-256
signature.json             optional: an Ed25519 signature over manifest.json
integrations/<key>.yaml    an OpenAPI spec (plus optional <key>.overlay.json)
workflows/<key>.yaml       an HWF workflow
forms/<key>.json           a form: { name, schema, ui }
scripts/<key>.json         a script's name, language and parameters, with scripts/<key>.ps1, .py or .js
templates/<key>.yaml       a template, in the built-in templates' format
datasets/<key>.json        a dataset its workflows write to: { name, description, columns, primaryKey }
docs/…                     anything you'd like to include for readers
```

The manifest's `key` is `<owner>/<name>` (lowercase, e.g. `contoso/onboarding`), and its `version` is semver. Because the manifest lists every file's hash and the signature covers the manifest's exact bytes, any change to any file is caught.

**Reading a pack is strict.** It must be at most 20 MB and 500 files. Every path must stay in those folders, every listed file must be there with its hash, and there can be no files the manifest doesn't list.

## Signing and trust

```sh
huxley pack keygen --out contoso-packs.key     # keep this file secret
# prints hxpk_…: the public key instances can trust
```

A pack is one of:

- **trusted**: signed by a key the instance trusts (Huxley's, or one in `packs.trusted_keys`)
- **signed by an unknown key**
- **unsigned**

A broken signature fails. Installing any pack shows what it adds and needs an admin's approval, and packs that aren't trusted say so.

## Building one

For what makes a good pack (keys, versions, upgrades, what to leave out), see [Writing packs](./pack-authoring.md).

Make a folder with `pack.yaml`, which is the manifest without `files`, and the content folders:

```yaml
apiVersion: usehuxley.io/v1
kind: Pack
key: contoso/hello
name: Hello pack
version: 1.0.0
publisher: Contoso
description: A greeting workflow and its form.
contents:
  workflows: [hello]
  forms: [hello]
```

```sh
huxley pack build ./hello --out hello.hxpack --key contoso-packs.key
huxley pack inspect hello.hxpack --trust hxpk_…
huxley pack sign hello.hxpack --key contoso-packs.key   # sign (or re-sign) an existing pack
```
