import { describe, expect, it } from 'vitest';
import { formatDuration, parseDuration } from './duration';

describe('durations', () => {
	it('reads what people type', () => {
		expect(parseDuration('')).toBeNull();
		expect(parseDuration('45')).toBe(45);
		expect(parseDuration('30m')).toBe(30);
		expect(parseDuration('4h')).toBe(240);
		expect(parseDuration('1h 30m')).toBe(90);
		expect(parseDuration('1.5h')).toBe(90);
		expect(parseDuration('2d')).toBe(2880);
		expect(parseDuration('soon')).toBeNaN();
		expect(parseDuration('4 hours')).toBeNaN();
	});

	it('writes them back', () => {
		expect(formatDuration(null)).toBe('');
		expect(formatDuration(30)).toBe('30m');
		expect(formatDuration(240)).toBe('4h');
		expect(formatDuration(90)).toBe('1h 30m');
		expect(parseDuration(formatDuration(2400))).toBe(2400);
	});
});
