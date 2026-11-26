import { describe, expect, it } from 'vitest';
import { warrantyText } from './devices.ts';

describe('warrantyText', () => {
	const today = new Date('2026-06-01T12:00:00');

	it('says nothing without a date', () => {
		expect(warrantyText(null, today)).toBeNull();
	});

	it('flags an ended warranty', () => {
		expect(warrantyText('2026-05-01', today)?.tone).toBe('danger');
	});

	it('warns when it ends within two months', () => {
		expect(warrantyText('2026-07-15', today)?.tone).toBe('warning');
	});

	it('is calm about a long one', () => {
		const w = warrantyText('2028-01-01', today);
		expect(w?.tone).toBe('neutral');
		expect(w?.text).toMatch(/^Warranty until /);
	});
});
