import { describe, expect, it } from 'vitest';
import { statusTone } from './status.ts';

describe('portal ticket statuses', () => {
	it('shows open work as info and finished work quietly', () => {
		expect(statusTone('new')).toBe('info');
		expect(statusTone('pending')).toBe('warning');
		expect(statusTone('resolved')).toBe('success');
		expect(statusTone('closed')).toBe('neutral');
	});
});
