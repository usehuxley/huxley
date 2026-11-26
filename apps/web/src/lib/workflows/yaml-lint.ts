import { isNode, parseDocument } from 'yaml';
import type { CodeDiagnostic } from '@usehuxley/ui';
import type { Problem } from './editor/context.ts';

/** `spec.steps[0].with.x` → `["spec", "steps", 0, "with", "x"]`. */
export function pathParts(path: string): (string | number)[] {
	const parts: (string | number)[] = [];
	for (const piece of path.split('.')) {
		const match = /^([^[\]]*)((?:\[\d+\])*)$/.exec(piece);
		if (!match) {
			parts.push(piece);
			continue;
		}
		if (match[1]) parts.push(match[1]);
		for (const index of match[2].matchAll(/\[(\d+)\]/g)) parts.push(Number(index[1]));
	}
	return parts;
}

/** Where a path is in YAML source: the deepest part of it that exists (a
 * missing field points at its parent), or the start. */
export function rangeOf(text: string, path: string): { from: number; to: number } {
	const doc = parseDocument(text);
	const parts = pathParts(path);
	for (let n = parts.length; n > 0; n--) {
		const node: unknown = doc.getIn(parts.slice(0, n), true);
		if (isNode(node) && node.range) {
			return { from: node.range[0], to: Math.max(node.range[0] + 1, node.range[1]) };
		}
	}
	const first = text.indexOf('\n');
	return { from: 0, to: first === -1 ? text.length : first };
}

/** YAML that doesn't parse, as problems at their places; empty when it
 * parses. */
export function syntaxProblems(text: string): CodeDiagnostic[] {
	const doc = parseDocument(text);
	return doc.errors.map((e) => ({
		from: e.pos[0],
		to: Math.max(e.pos[0] + 1, e.pos[1]),
		severity: 'error' as const,
		message: e.message.split('\n')[0]
	}));
}

/** Everything wrong with a workflow's YAML: syntax first, then what the
 * server's validator says, each underlined where it is. */
export async function lintWorkflowYaml(
	text: string,
	check: (document: unknown) => Promise<Problem[]>
): Promise<CodeDiagnostic[]> {
	if (!text.trim()) return [];
	const syntax = syntaxProblems(text);
	if (syntax.length > 0) return syntax;
	const document: unknown = parseDocument(text).toJS();
	const problems = await check(document);
	return problems.map((p) => ({
		...rangeOf(text, p.path),
		severity: 'error' as const,
		message: p.path ? `${p.path}: ${p.message}` : p.message
	}));
}

/** JSON that doesn't parse, underlined where it stops. */
export function lintJson(text: string): CodeDiagnostic[] {
	if (!text.trim()) return [];
	try {
		JSON.parse(text);
		return [];
	} catch (e) {
		const message = e instanceof Error ? e.message : String(e);
		const pos = /position (\d+)/.exec(message);
		const lc = /line (\d+) column (\d+)/.exec(message);
		let at = text.length;
		if (pos) at = Number(pos[1]);
		else if (lc) {
			const before = text.split('\n').slice(0, Number(lc[1]) - 1);
			at = before.reduce((n, l) => n + l.length + 1, 0) + Number(lc[2]) - 1;
		}
		at = Math.min(at, Math.max(0, text.length - 1));
		return [{ from: at, to: Math.min(at + 1, text.length), severity: 'error', message }];
	}
}
