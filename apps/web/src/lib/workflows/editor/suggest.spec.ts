import { describe, expect, it } from 'vitest';
import { complete, namesFor } from './suggest.ts';

const doc = {
	spec: {
		inputs: { type: 'object', properties: { email: { type: 'string' } } },
		steps: [
			{ id: 'lookup', type: 'http.request', with: { url: 'x' } },
			{
				id: 'check',
				type: 'if',
				condition: '{{ true }}',
				then: [{ id: 'note', type: 'set_var', vars: { seen: true } }],
				else: []
			},
			{
				id: 'each',
				type: 'foreach',
				as: 'user',
				items: '{{ [] }}',
				steps: [
					{ id: 'inner', type: 'transform', output: 1 },
					{ id: 'after_inner', type: 'transform', output: 2 }
				]
			},
			{
				id: 'both',
				type: 'parallel',
				branches: [
					{ id: 'a', steps: [{ id: 'left', type: 'transform', output: 1 }] },
					{ id: 'b', steps: [{ id: 'right', type: 'transform', output: 2 }] }
				]
			},
			{ id: 'last', type: 'transform', output: 3 }
		]
	}
};

const labels = (path: (string | number)[]) => namesFor(doc, path).map((s) => s.label);

describe('namesFor', () => {
	it('offers inputs, earlier steps and what they contain, but not loop bodies', () => {
		const names = labels(['spec', 'steps', 4]);
		expect(names).toContain('inputs.email');
		expect(names).toContain('steps.lookup.output');
		expect(names).toContain('steps.note.output');
		expect(names).toContain('steps.each.output');
		expect(names).not.toContain('steps.inner.output');
		expect(names).not.toContain('steps.last.output');
		expect(names).toContain('run.vars.seen');
	});

	it('inside a loop: the item, the loop, and earlier steps in the body', () => {
		const names = labels(['spec', 'steps', 2, 'steps', 1]);
		expect(names).toContain('user');
		expect(names).toContain('loop.index');
		expect(names).toContain('steps.inner.output');
		expect(names).not.toContain('steps.each.output');
		expect(labels(['spec', 'steps', 2])).not.toContain('user');
	});

	it('a parallel branch sees neither the other branch nor branch ids', () => {
		const names = labels(['spec', 'steps', 3, 'branches', 1, 'steps', 0]);
		expect(names).not.toContain('steps.left.output');
		expect(names).not.toContain('steps.a.output');
		expect(names).toContain('steps.lookup.output');
	});
});

describe('complete', () => {
	const names = namesFor(doc, ['spec', 'steps', 4]);

	it('completes the word being typed inside {{', () => {
		const text = '{{ steps.lo';
		const c = complete(text, text.length, names);
		expect(c?.from).toBe(3);
		expect(c?.items.map((i) => i.label)).toContain('steps.lookup.output');
	});

	it('offers filters after a pipe', () => {
		const text = '{{ inputs.email | up';
		const c = complete(text, text.length, names);
		expect(c?.items.map((i) => i.label)).toEqual(['upper']);
	});

	it('offers everything right after {{, and nothing outside an expression', () => {
		expect(complete('{{ ', 3, names)?.items.length).toBeGreaterThan(0);
		expect(complete('plain ste', 9, names)).toBeNull();
		expect(complete('{{ a }} ste', 11, names)).toBeNull();
	});
});
