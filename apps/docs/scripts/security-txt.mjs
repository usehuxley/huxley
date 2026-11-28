// Publishes /.well-known/security.txt (RFC 9116) with the site: where to
// report a vulnerability in Huxley. Its Expires date is a year from each
// build, so it stays current while the site keeps deploying.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const target = join(here, "..", ".vitepress", "dist", ".well-known");
const expires = new Date(Date.now() + 365 * 24 * 3600 * 1000);
expires.setUTCHours(0, 0, 0, 0);
mkdirSync(target, { recursive: true });
writeFileSync(
  join(target, "security.txt"),
  [
    "# Report vulnerabilities in Huxley privately, never in a public issue.",
    "Contact: https://github.com/usehuxley/huxley/security/advisories/new",
    "Contact: mailto:security@usehuxley.io",
    `Expires: ${expires.toISOString().replace(".000Z", "Z")}`,
    "Policy: https://github.com/usehuxley/huxley/blob/main/SECURITY.md",
    "Canonical: https://docs.usehuxley.io/.well-known/security.txt",
    "Preferred-Languages: en",
    "",
  ].join("\n"),
);
console.log("wrote .well-known/security.txt");
