import { describe, expect, it } from 'vitest';
import { fromYaml, keyFromName, orderDocument, starter, toYaml } from './yaml.ts';

describe('workflow YAML', () => {
	it('round-trips documents in HWF key order', () => {
		const doc = starter('greet', 'Greet');
		// As JSON storage might return it: keys shuffled.
		const shuffled: unknown = JSON.parse(
			JSON.stringify({
				spec: doc.spec,
				metadata: doc.metadata,
				kind: doc.kind,
				apiVersion: doc.apiVersion
			})
		);
		const text = toYaml(shuffled);
		expect(text.indexOf('apiVersion')).toBeLessThan(text.indexOf('spec'));
		expect(text.indexOf('id: greet')).toBeLessThan(text.indexOf('type: transform'));
		const parsed = fromYaml(text);
		expect('document' in parsed && parsed.document).toEqual(orderDocument(doc));
	});

	it('orders nested blocks', () => {
		const doc = orderDocument({
			spec: {
				steps: [
					{
						else: [],
						condition: '{{ x }}',
						type: 'if',
						then: [{ output: 1, type: 'transform', id: 'b' }],
						id: 'a'
					}
				]
			}
		}) as { spec: { steps: Record<string, unknown>[] } };
		expect(Object.keys(doc.spec.steps[0])).toEqual(['id', 'type', 'condition', 'then', 'else']);
		expect(Object.keys((doc.spec.steps[0].then as Record<string, unknown>[])[0])).toEqual([
			'id',
			'type',
			'output'
		]);
	});

	it('reports YAML errors and refuses alias bombs', () => {
		expect('error' in fromYaml('a: [1, 2')).toBe(true);
		const bomb = ['a: &a [x,x,x,x,x,x,x,x,x,x]'];
		for (let i = 0; i < 8; i++) {
			const prev = i === 0 ? 'a' : `l${i - 1}`;
			bomb.push(`l${i}: &l${i} [${Array(10).fill(`*${prev}`).join(',')}]`);
		}
		expect('error' in fromYaml(bomb.join('\n'))).toBe(true);
	});

	it('makes keys from names', () => {
		expect(keyFromName('Onboard New User!')).toBe('onboard-new-user');
		expect(keyFromName('123 Go')).toBe('go');
	});
});
