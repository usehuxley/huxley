import { compile } from 'json-schema-to-typescript';

const SHAPING = [
	'type',
	'$ref',
	'oneOf',
	'anyOf',
	'allOf',
	'enum',
	'const',
	'properties',
	'items',
	'additionalProperties'
];

/** Fields that accept any value (no constraints) are `unknown`, not objects. */
function anyValueIsUnknown(schema: unknown): unknown {
	if (Array.isArray(schema)) return schema.map(anyValueIsUnknown);
	if (!schema || typeof schema !== 'object') return schema;
	const out: Record<string, unknown> = {};
	for (const [k, v] of Object.entries(schema)) {
		out[k] =
			k === 'properties' || k === '$defs'
				? Object.fromEntries(
						Object.entries(v as Record<string, unknown>).map(([name, s]) => [
							name,
							anyValueIsUnknown(s)
						])
					)
				: anyValueIsUnknown(v);
	}
	if (!SHAPING.some((k) => k in out)) out.tsType = 'unknown';
	return out;
}

/** The TypeScript for the HWF schema. */
export async function generate(schema: Record<string, unknown>): Promise<string> {
	const { tsType: _root, ...prepared } = anyValueIsUnknown(schema) as Record<string, unknown>;
	return compile(prepared as Parameters<typeof compile>[0], 'Workflow', {
		bannerComment:
			'/* Generated from schemas/hwf/v1.json by `pnpm generate`. Do not edit: HWF v1 only grows (ADR-042). */',
		additionalProperties: false,
		unreachableDefinitions: true,
		style: { useTabs: true, singleQuote: true, printWidth: 100 },
		strictIndexSignatures: false
	});
}
