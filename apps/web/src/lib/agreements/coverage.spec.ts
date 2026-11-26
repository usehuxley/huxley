import { describe, expect, it } from 'vitest';
import { hours, kindLabel, OUTCOME } from './coverage.ts';

describe('agreement helpers', () => {
	it('reads minutes as hours', () => {
		expect(hours(90)).toBe('1.5 h');
		expect(hours(20)).toBe('0.33 h');
		expect(hours(600)).toBe('10 h');
	});
	it('labels kinds and outcomes', () => {
		expect(kindLabel('block_hours')).toBe('Block hours');
		expect(OUTCOME.partly_covered.label).toBe('Partly covered');
	});
});
