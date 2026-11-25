import { describe, expect, it } from 'vitest';
import { addWeeks, mondayOf, weekDays } from './week';

describe('weeks', () => {
	it('start on Monday', () => {
		expect(mondayOf('2026-11-25')).toBe('2026-11-23'); // a Wednesday
		expect(mondayOf('2026-11-29')).toBe('2026-11-23'); // Sunday stays in its week
		expect(mondayOf('2026-11-23')).toBe('2026-11-23');
	});

	it('have seven days, across months', () => {
		expect(weekDays('2026-11-30')).toEqual([
			'2026-11-30',
			'2026-12-01',
			'2026-12-02',
			'2026-12-03',
			'2026-12-04',
			'2026-12-05',
			'2026-12-06'
		]);
	});

	it('move by weeks', () => {
		expect(addWeeks('2026-11-23', 1)).toBe('2026-11-30');
		expect(addWeeks('2026-11-23', -1)).toBe('2026-11-16');
	});
});
