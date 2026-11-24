import { describe, expect, it } from 'vitest';
import { ago, categoryTone, priorityOf } from './labels';

describe('ticket labels', () => {
	it('describes how long ago', () => {
		const now = new Date('2026-11-25T12:00:00Z');
		expect(ago('2026-11-25T11:59:30Z', now)).toBe('just now');
		expect(ago('2026-11-25T11:55:00Z', now)).toBe('5 minutes ago');
		expect(ago('2026-11-25T09:00:00Z', now)).toBe('3 hours ago');
		expect(ago('2026-11-24T12:00:00Z', now)).toBe('yesterday');
	});

	it('picks tones and priorities', () => {
		expect(categoryTone('pending')).toBe('warning');
		expect(categoryTone('closed')).toBe('neutral');
		expect(priorityOf('p1').short).toBe('P1');
		expect(priorityOf('nope').value).toBe('p3');
	});
});
