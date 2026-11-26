# Huxley workflows in TypeScript

A starting point for a repository of workflows written with `@usehuxley/sdk`. It holds the built-in timesheet reminders pack, rewritten with the SDK; a test checks they compile to exactly the pack's YAML.

```sh
npm install
huxley workflows codegen --out integrations.ts   # types for your integrations (commit it)
npm run check                                    # type-check
npx huxley-sdk build . --out dist                # TypeScript → HWF JSON
huxley workflows validate dist
huxley workflows push dist --publish
```

- **Layout:** one workflow per `*.workflow.ts` (its default export), forms in `*.form.ts`. Folders become workflow folders in Huxley.
- **CI:** copy `github-actions.yml` to `.github/workflows/huxley.yml`. Pull requests build, validate, test and diff; pushes to `main` publish.
- **Docs:** [TypeScript SDK](../../docs/sdk.md).
