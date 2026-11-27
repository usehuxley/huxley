# Security

How Huxley protects itself and its data. Reporting a vulnerability: see [SECURITY.md](https://github.com/usehuxley/huxley/blob/main/SECURITY.md).

## Hostile input

Huxley parses input it doesn't control: workflow documents, OpenAPI specs that admins import, packs from uploads and registries, inbound email, expressions (some of whose data arrives from webhooks and email), and SCIM requests from identity providers. Each parser has hard limits, and tests feed each one random and malicious input to check they hold. Every parser must answer with a value or an error. None may panic, hang, exhaust memory or overflow the stack.

| Parser | Limits | Tested against |
|---|---|---|
| Workflow documents (YAML, JSON) | Size cap; YAML 1.2 rules; the YAML library's alias and depth budgets | "Billion laughs" alias bombs; 200,000-deep nesting; random and YAML-shaped text |
| OpenAPI imports | Size cap; same YAML budgets | Alias bombs; deep nesting; random text; spec-shaped documents with dangling and self-referencing `$ref`s |
| Packs (`.hxpack`) | 20 MB in total and 500 files, enforced while reading, whatever sizes the zip claims; every file hash-checked | A 200 MB zip bomb in about 200 KB; random bytes; a real pack with any single bit flipped (refused, or byte-for-byte unchanged) |
| Inbound email | Body size caps | 2,000-deep nested multiparts; random bytes; header-shaped messages |
| Expressions | Fuel (instructions), recursion depth, output size | Billion-character strings, billion-item ranges, nested loops, unbounded recursion; random and template-shaped input |
| SCIM filters and PATCH | `eq` filters only; PATCH applied to a copy, then validated | Random filters; random operations with arbitrary JSON values and paths |

The tests are the `hostile` tests in each parser's crate (`crates/huxley-*/tests/hostile.rs`, and the `random` tests in `scim_routes.rs`). They run with the rest of the test suite. Property tests run 512 random cases per check on every run, each run with fresh cases.
