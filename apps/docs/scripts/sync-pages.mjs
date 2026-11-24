// Copies the public pages from the repository's docs/ into src/ (which
// isn't committed), so the site renders the same files the repo shows.
// Internal documents (design doc, plan, ADRs) stay out.
import { cpSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const INTERNAL = new Set([
	'DESIGN_DOCUMENT.md',
	'IMPLEMENTATION_PLAN.md',
	'PROJECT_OVERVIEW.md',
	'api-conventions.md',
	'performance.md',
	'adr'
]);

const here = dirname(fileURLToPath(import.meta.url));
const from = join(here, '..', '..', '..', 'docs');
const to = join(here, '..', 'src');
rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
for (const name of readdirSync(from)) {
	if (INTERNAL.has(name)) continue;
	cpSync(join(from, name), join(to, name), { recursive: statSync(join(from, name)).isDirectory() });
}
console.log('synced docs/ into src/');
