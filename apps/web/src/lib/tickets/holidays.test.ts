import { describe, expect, it } from 'vitest';
import { formatHolidays, parseHolidays } from './holidays';

describe('holidays', () => {
	it('reads a date and a name a line, skipping blanks', () => {
		expect(
			parseHolidays('2026-12-25 Christmas Day\n\n  2026-12-26   \n2027-01-01 New Year')
		).toEqual([
			{ date: '2026-12-25', name: 'Christmas Day' },
			{ date: '2026-12-26', name: '' },
			{ date: '2027-01-01', name: 'New Year' }
		]);
		expect(parseHolidays('')).toEqual([]);
	});

	it('writes them back the same way', () => {
		const list = [
			{ date: '2026-12-25', name: 'Christmas Day' },
			{ date: '2026-12-26', name: '' }
		];
		expect(parseHolidays(formatHolidays(list))).toEqual(list);
	});
});
