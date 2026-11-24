# Huxley docs site

The documentation at **docs.usehuxley.com**, built with [VitePress](https://vitepress.dev) from the pages in the repository's [`docs/`](../../docs) folder. There's one copy of each page: it's readable on GitHub and published here. Internal documents (the design doc, the plan, ADRs) aren't published; links to them, and to files outside `docs/`, point to GitHub.

```sh
pnpm --filter @usehuxley/docs dev      # http://localhost:5173
pnpm --filter @usehuxley/docs build    # → apps/docs/.vitepress/dist
```

How the build works:

- **Pages:** the build copies the public pages into `src/` (`scripts/sync-pages.mjs`; not committed). Add a new page to the `sidebar` in `.vitepress/config.mts`.
- **Schema:** the build also publishes the HWF JSON Schema at `/schemas/hwf/v1.json` (`scripts/copy-schemas.mjs`). [`docs/public/_headers`](../../docs/public/_headers) serves it with an open CORS header, so editors can fetch it.
- **Expressions:** `{{ … }}` is shown as written in page text and inline code, never read as Vue.

## Deploying (Cloudflare Pages)

[`.github/workflows/docs.yml`](../../.github/workflows/docs.yml) builds the site on pull requests, and builds and deploys it on every push to `main`. It uses Cloudflare's Direct Upload: CI builds the site with the repository's own toolchain, then wrangler uploads `dist`. That avoids depending on the Pages build image supporting this repo's pnpm version.

One-time setup:

1. **In Cloudflare:**
   - Create a Pages project with **Direct Upload**, named `huxley-docs` (or any name; see step 3).
   - Under **Custom domains**, add `docs.usehuxley.com`.
2. **Create an API token** with the **Account → Cloudflare Pages → Edit** permission.
3. **In GitHub** (repository settings):
   - **Secrets:** `CLOUDFLARE_API_TOKEN` (the token) and `CLOUDFLARE_ACCOUNT_ID`.
   - **Variable (optional):** `CLOUDFLARE_PAGES_PROJECT`, if the project isn't called `huxley-docs`.
4. **Deploy:** run the **Docs** workflow (or push to `main`).

Until the secrets exist, the workflow only builds.

**The schema's `$id` is `https://usehuxley.com/schemas/hwf/v1.json`.** If usehuxley.com is a different site, make that URL serve the docs copy. Either:

- add a Cloudflare **redirect rule** from `usehuxley.com/schemas/*` to `https://docs.usehuxley.com/schemas/${1}`, or
- publish `schemas/` with the main site too.
