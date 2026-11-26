#!/usr/bin/env node
/**
 * `huxley-sdk build [src] [--out dist] [--forms out/forms]`: compiles
 * `*.workflow.ts` and `*.form.ts` files to HWF JSON (Node 24 runs the
 * TypeScript directly). Then `huxley workflows validate dist` and
 * `huxley workflows push dist`.
 */
import { parseArgs } from 'node:util';
import { build } from './build.ts';

const { values, positionals } = parseArgs({
	allowPositionals: true,
	options: { out: { type: 'string', default: 'dist' }, forms: { type: 'string' } }
});
const [command, src = '.'] = positionals;
if (command !== 'build') {
	console.error('usage: huxley-sdk build [src] [--out dist] [--forms forms-dir]');
	process.exit(2);
}
try {
	const built = await build(src, values.out, values.forms);
	for (const f of built.workflows) console.log(`workflow  ${f}`);
	for (const f of built.forms) console.log(`form      ${f}`);
	console.log(`built ${built.workflows.length} workflows, ${built.forms.length} forms`);
} catch (e) {
	console.error(e instanceof Error ? e.message : e);
	process.exit(1);
}
