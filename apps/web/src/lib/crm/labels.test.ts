import { describe, expect, it } from 'vitest';
import { hoursSummary } from './labels.ts';

describe('opening hours summary', () => {
	it('groups days with the same hours', () => {
		const week = {
			mon: [['09:00', '17:00']],
			tue: [['09:00', '17:00']],
			wed: [['09:00', '17:00']],
			thu: [['09:00', '17:00']],
			fri: [['09:00', '16:00']]
		};
		expect(hoursSummary(week)).toBe('Mon–Thu 09:00–17:00; Fri 09:00–16:00');
		expect(
			hoursSummary({
				sat: [
					['10:00', '12:00'],
					['13:00', '15:00']
				]
			})
		).toBe('Sat 10:00–12:00, 13:00–15:00');
		expect(hoursSummary({})).toBe('No opening hours');
	});
});
