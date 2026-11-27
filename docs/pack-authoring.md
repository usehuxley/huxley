# Writing packs

A pack shares automation between Huxley instances: an integration with the workflows that use it, a form and the workflow it starts, or a whole onboarding process. This guide covers making one others can install with confidence. [Packs](./packs.md) covers the format and the commands.

## Shape it

- **One job per pack.** "Onboard a Microsoft 365 user" is a pack; "everything we built" isn't. People review everything a pack adds before installing, so a focused pack is easier to say yes to.
- **Bring what the workflows need.** If a workflow calls an integration, include the integration. If it writes to a dataset, include the dataset's definition. If a form starts it, include the form.
- **Name things for their job.** Keys (`onboard-user`, `msgraph`) are how upgrades find what a pack installed, so pick them once and keep them. Display names can change.

## What never goes in

- **Credentials and connections.** Exports leave them out. Don't add them by hand either: installers create their own connections.
- **Customer data.** No dataset rows, ticket text, or names and addresses in examples.
- **Hard-coded tenants or hosts** belonging to one customer. Use integration server variables and connections, and workflow inputs.

## Make it safe to review

Reviewers see every host a pack's integrations and HTTP steps reach, and every script it adds. Keep both lists short and obvious:

- **Prefer integration actions to raw `http.request` steps.** An integration's operations are typed, documented, and use the installer's connection.
- **Keep scripts small** and say in their names what they do. They run in sandboxes on the installer's runners.
- **Explain anything surprising** in `docs/`, for example why a workflow needs a host.

## Versions and upgrades

The `version` is semver:

- **Patch** (`1.0.1`): fixes that don't change inputs or outputs.
- **Minor** (`1.1.0`): new things, such as an optional input, a new workflow or a dataset column.
- **Major** (`2.0.0`): anything an installer must act on, such as a required input, a removed workflow or a changed dataset key.

Installing a newer version upgrades in place:

- **Workflows** get a new draft, which installers review and publish.
- **Scripts** get a new version.
- **Integration specs** are updated, and installers' own overlay edits stay.
- **Datasets** gain columns, and their rows are kept.
- **Removed items** (in the old version, not the new one) are left alone, never deleted. If something should go, say so in your release notes.

## Test it

```sh
huxley pack build ./my-pack --out my-pack-1.0.0.hxpack
huxley pack inspect my-pack-1.0.0.hxpack
```

Then install it into a test organization, connect the integration, publish the workflows and run them. Check the review screen reads the way you'd want an installer to see it. Then install the next version over it, to check the upgrade.

## Sign it

```sh
huxley pack keygen --out my-packs.key        # once; keep it secret and backed up
huxley pack build ./my-pack --out my-pack-1.0.0.hxpack --key my-packs.key
```

Publish the public key it prints (`hxpk_…`) where installers can check it: your website, your README. Instances that list it in `packs.trusted_keys` show your packs as **Signed, trusted**. Use one key per publisher, not per pack. If a key leaks, make a new one and tell installers to stop trusting the old one.

## Share it

- **With a few instances:** send the `.hxpack`, which they upload.
- **With many,** or your own clients: host a registry. It's a folder on any static web server, indexed with `huxley pack index` (see [Hosting a registry](./packs.md#hosting-a-registry)).
