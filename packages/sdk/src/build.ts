/**
 * Builds workflow and form files from TypeScript: each `*.workflow.ts`
 * default-exports a workflow (or a list of them) and each `*.form.ts` a
 * form. Output is JSON, written deterministically: the same sources always
 * give the same bytes.
 */

import { mkdir, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { FormDocument } from './form.ts';
import type { Workflow } from './index.ts';

export interface Built {
	/** Files written, relative to the output folder. */
	workflows: string[];
	forms: string[];
}

async function walk(dir: string): Promise<string[]> {
	const out: string[] = [];
	for (const entry of await readdir(dir, { withFileTypes: true })) {
		if (entry.name.startsWith('.') || entry.name === 'node_modules') continue;
		const path = join(dir, entry.name);
		if (entry.isDirectory()) out.push(...(await walk(path)));
		else if (/\.(workflow|form)\.ts$/.test(entry.name)) out.push(path);
	}
	return out.sort();
}

/** The JSON a document is written as. */
export function render(doc: unknown): string {
	return `${JSON.stringify(doc, null, 2)}\n`;
}

function isWorkflow(v: unknown): v is Workflow {
	return (
		typeof v === 'object' &&
		v !== null &&
		(v as { apiVersion?: unknown }).apiVersion === 'usehuxley.io/v1' &&
		(v as { kind?: unknown }).kind === 'Workflow'
	);
}

/**
 * Builds every source under `src` into `out`: workflows keep their folders
 * (so `huxley workflows push` files them there), named by their key; forms
 * go into `formsOut` when given.
 */
export async function build(src: string, out: string, formsOut?: string): Promise<Built> {
	const root = resolve(src);
	const built: Built = { workflows: [], forms: [] };
	const keys = new Map<string, string>();
	for (const file of await walk(root)) {
		const mod = (await import(pathToFileURL(file).href)) as { default?: unknown };
		const rel = relative(root, file);
		if (file.endsWith('.form.ts')) {
			const f = mod.default as FormDocument | undefined;
			if (!f?.key || !f.schema) throw new Error(`${rel}: the default export isn't a form()`);
			if (!formsOut) continue;
			const { key, ...content } = f;
			const target = join(resolve(formsOut), `${key}.json`);
			await mkdir(dirname(target), { recursive: true });
			await writeFile(target, render(content));
			built.forms.push(relative(resolve(formsOut), target));
			continue;
		}
		const docs = Array.isArray(mod.default) ? (mod.default as unknown[]) : [mod.default];
		for (const doc of docs) {
			if (!isWorkflow(doc)) throw new Error(`${rel}: the default export isn't a workflow()`);
			const key = doc.metadata.key;
			const seen = keys.get(key);
			if (seen) throw new Error(`${rel}: the key ${key} is also used by ${seen}`);
			keys.set(key, rel);
			const target = join(resolve(out), dirname(rel), `${key}.json`);
			await mkdir(dirname(target), { recursive: true });
			await writeFile(target, render(doc));
			built.workflows.push(relative(resolve(out), target));
		}
	}
	return built;
}
