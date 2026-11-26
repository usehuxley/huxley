import type { CodeCompletion, CodeDiagnostic, CodeLanguage } from '@usehuxley/ui';

/** The editor language for a script's language. */
export const editorLanguage = (language: string): CodeLanguage =>
	language === 'pwsh' ? 'powershell' : language === 'javascript' ? 'javascript' : 'python';

type Schema = {
	properties?: Record<string, { type?: string; description?: string; title?: string }>;
};

/** What a script can use: its parameters, as each language receives them
 * (PowerShell `param(...)` arguments, Python's `inputs` dict, JavaScript's
 * argument object). */
export function scriptWords(language: string, parameters: unknown): CodeCompletion[] {
	const props =
		typeof parameters === 'object' && parameters !== null
			? ((parameters as Schema).properties ?? {})
			: {};
	const params = Object.entries(props).map(([name, p]) => ({
		name,
		detail: p.type ?? 'parameter',
		info: p.description ?? p.title
	}));
	if (language === 'pwsh') {
		return params.map((p) => ({
			label: `$${p.name}`,
			detail: p.detail,
			info: p.info,
			type: 'variable'
		}));
	}
	if (language === 'javascript') {
		return [
			...params.map((p) => ({
				label: p.name,
				detail: p.detail,
				info: p.info,
				type: 'variable' as const
			})),
			{ label: 'globalThis.result', detail: 'the output', type: 'variable' }
		];
	}
	return [
		...params.map((p) => ({
			label: `inputs["${p.name}"]`,
			detail: p.detail,
			info: p.info,
			type: 'variable' as const
		})),
		{ label: 'inputs', detail: 'the parameters', type: 'variable' },
		{ label: 'result', detail: 'the output', type: 'variable' }
	];
}

/** Where JSON stops parsing, from the engine's message ("… at position
 * 42" or "… (line 3 column 5)"). */
function offset(text: string, message: string): number {
	const pos = /position (\d+)/.exec(message);
	if (pos) return Number(pos[1]);
	const lc = /line (\d+) column (\d+)/.exec(message);
	if (lc) {
		const before = text.split('\n').slice(0, Number(lc[1]) - 1);
		return before.reduce((n, l) => n + l.length + 1, 0) + Number(lc[2]) - 1;
	}
	return text.length;
}

/** Problems with a parameters JSON Schema: bad JSON, or not an object
 * schema. */
export function lintParameters(text: string): CodeDiagnostic[] {
	if (!text.trim()) return [];
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch (e) {
		const message = e instanceof Error ? e.message : String(e);
		const at = Math.min(offset(text, message), Math.max(0, text.length - 1));
		return [{ from: at, to: Math.min(at + 1, text.length), severity: 'error', message }];
	}
	if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
		return [
			{
				from: 0,
				to: text.length,
				severity: 'error',
				message: 'The parameters are a JSON Schema object.'
			}
		];
	}
	const type = (parsed as { type?: unknown }).type;
	if (type !== undefined && type !== 'object') {
		return [
			{
				from: 0,
				to: text.length,
				severity: 'warning',
				message: 'Parameters are named, so the schema’s type is "object".'
			}
		];
	}
	return [];
}
