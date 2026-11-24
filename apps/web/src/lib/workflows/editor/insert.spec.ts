import { describe, expect, it } from 'vitest';
import { insertStep, listAt } from './insert.ts';

describe('inserting from the canvas', () => {
	it('inserts at the "+" position and returns the new path', () => {
		const doc = {
			spec: {
				steps: [
					{ id: 'a', type: 'transform' },
					{ id: 'b', type: 'transform' }
				]
			}
		};
		const path = insertStep(doc, { list: ['spec', 'steps'], index: 1, label: '' }, 'transform');
		expect(path).toEqual(['spec', 'steps', 1]);
		expect(doc.spec.steps.map((s) => s.id)).toEqual(['a', 'transform_1', 'b']);
	});

	it('creates a missing optional block (an else, a finally) on first use', () => {
		const doc: Record<string, unknown> = {
			spec: { steps: [{ id: 't', type: 'try', steps: [], catch: [] }] }
		};
		insertStep(doc, { list: ['spec', 'steps', 0, 'finally'], index: 0, label: '' }, 'core.notify');
		expect(listAt(doc, ['spec', 'steps', 0, 'finally'])).toHaveLength(1);
		// And a document with no spec at all.
		const empty: Record<string, unknown> = {};
		insertStep(empty, { list: ['spec', 'steps'], index: 0, label: '' }, 'transform');
		expect(listAt(empty, ['spec', 'steps'])).toHaveLength(1);
	});
});
