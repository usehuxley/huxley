import { describe, expect, it } from 'vitest';
import { diffLines } from './diff.ts';

describe('diffLines', () => {
	it('marks added, removed and kept lines', () => {
		const diff = diffLines('a\nb\nc\nd', 'a\nc\nx\nd');
		expect(diff?.map((l) => `${l.kind}${l.text}`)).toEqual([' a', '-b', ' c', '+x', ' d']);
	});

	it('handles identical and empty inputs', () => {
		expect(diffLines('same', 'same')).toEqual([{ kind: ' ', text: 'same' }]);
		expect(diffLines('', 'new')?.map((l) => l.kind)).toEqual(['-', '+']);
	});

	it('refuses very long inputs', () => {
		const long = Array.from({ length: 6000 }, (_, i) => String(i)).join('\n');
		expect(diffLines(long, 'x')).toBeNull();
	});
});
