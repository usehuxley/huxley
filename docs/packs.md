# Packs

A **pack** bundles integrations, workflows, forms, scripts and templates so they can be shared between instances, by Huxley, MSPs and the community (design doc §6.6, ADR-028). Installing packs in the app (with a review of what they add), exporting them, and the built-in packs come next. This page covers the format and the `huxley pack` command.

## The format

A `.hxpack` file is a zip:

```text
manifest.json              what it is, what it adds, and every file's SHA-256
signature.json             optional: an Ed25519 signature over manifest.json
integrations/<key>.yaml    an OpenAPI spec (plus optional <key>.overlay.json)
workflows/<key>.yaml       an HWF workflow
forms/<key>.json           a form: { name, schema, ui }
scripts/<key>.json         a script's name, language and parameters, with scripts/<key>.ps1 or .py
templates/<key>.yaml       a template, in the built-in templates' format
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

Make a folder with `pack.yaml`, which is the manifest without `files`, and the content folders:

```yaml
apiVersion: usehuxley.com/v1
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
```
