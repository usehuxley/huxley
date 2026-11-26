/**
 * References into a run's context, for `{{ … }}` expressions:
 * `ref.inputs.name`, `ref.steps.find.output.people`, `ref.vars.count`,
 * `ref.trigger.body`, `ref.error.message`, or a loop variable with
 * `ref.at('person').recipient`.
 *
 * Used as a whole value, a reference becomes `{{ path }}`. Inside the
 * `expr` tag it's a bare path in one expression; inside the `text` tag (or
 * a template string) it becomes `{{ path }}` in the text.
 */

const PATH = Symbol('huxley.ref.path');

/** A reference: any property is a deeper reference. */
export type Ref = { readonly [key: string]: Ref } & { readonly [PATH]: string } & string;

/** Whether a value is a reference. */
export function isRef(value: unknown): value is Ref {
	return (
		(typeof value === 'object' || typeof value === 'function') &&
		value !== null &&
		typeof (value as { [PATH]?: unknown })[PATH] === 'string'
	);
}

/** A reference's path, e.g. `steps.find.output.people`. */
export function pathOf(r: Ref): string {
	return r[PATH];
}

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*$/;

function join(path: string, key: string): string {
	if (/^\d+$/.test(key)) return `${path}[${key}]`;
	if (IDENT.test(key)) return path ? `${path}.${key}` : key;
	return `${path}[${JSON.stringify(key)}]`;
}

/** A reference at `path`. */
export function refAt(path: string): Ref {
	const target = function () {} as unknown as object;
	return new Proxy(target, {
		get(_, key) {
			if (key === PATH) return path;
			if (key === Symbol.toPrimitive || key === 'toString' || key === 'toJSON') {
				return () => `{{ ${path} }}`;
			}
			if (typeof key === 'symbol') return undefined;
			return refAt(join(path, key));
		}
	}) as Ref;
}

/** The run's context: inputs, steps, vars, trigger, run, error, secrets… */
export const ref = new Proxy({} as Record<string, Ref> & { at(name: string): Ref }, {
	get(_, key) {
		if (key === 'at') return (name: string) => refAt(name);
		if (typeof key === 'symbol') return undefined;
		return refAt(key);
	}
});

function render(strings: TemplateStringsArray, values: unknown[], inside: boolean): string {
	let out = strings[0];
	values.forEach((v, i) => {
		if (isRef(v)) out += inside ? pathOf(v) : `{{ ${pathOf(v)} }}`;
		else out += inside ? JSON.stringify(v) : String(v);
		out += strings[i + 1];
	});
	return out;
}

/**
 * One expression: `expr\`${ref.steps.find.output.people} | length\`` is
 * `{{ steps.find.output.people | length }}`. Plain values are inlined as
 * JSON literals.
 */
export function expr(strings: TemplateStringsArray, ...values: unknown[]): string {
	return `{{ ${render(strings, values, true).trim()} }}`;
}

/** Text with expressions in it: `text\`Hello ${ref.inputs.name}\``. */
export function text(strings: TemplateStringsArray, ...values: unknown[]): string {
	return render(strings, values, false);
}
