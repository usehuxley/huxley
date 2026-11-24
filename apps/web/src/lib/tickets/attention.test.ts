import { describe, expect, it } from 'vitest';
import { ATTENTION, nextDue } from './attention';

describe('attention', () => {
	it('labels every reason the API gives', () => {
		for (const reason of ['breached', 'replied', 'due_soon', 'new']) {
			expect(ATTENTION[reason]?.label).toBeTruthy();
		}
	});

	it('is due for the first response until there is one, then resolution', () => {
		const t = { responseDueAt: '2026-12-01T10:00:00Z', resolutionDueAt: '2026-12-02T10:00:00Z' };
		expect(nextDue(t)).toBe('2026-12-01T10:00:00Z');
		expect(nextDue({ ...t, firstResponseAt: '2026-12-01T09:00:00Z' })).toBe('2026-12-02T10:00:00Z');
		expect(nextDue({})).toBeNull();
	});
});
