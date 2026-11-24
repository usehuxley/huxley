import { describe, expect, it } from 'vitest';
import { clockLines, missed } from './sla';

const now = new Date('2026-11-26T12:00:00Z');
const base = { slaPolicy: 'Standard', slaPaused: false };

describe('SLA clocks', () => {
	it('shows nothing without a policy', () => {
		expect(clockLines({ slaPaused: false }, now)).toEqual([]);
	});

	it('shows due, overdue, met and paused', () => {
		expect(
			clockLines(
				{
					...base,
					responseDueAt: '2026-11-26T14:00:00Z',
					resolutionDueAt: '2026-11-26T11:00:00Z'
				},
				now
			)
		).toEqual([
			{ label: 'First response', text: 'Due in 2 hours', tone: 'neutral' },
			{ label: 'Resolution', text: 'Overdue (due 1 hour ago)', tone: 'danger' }
		]);
		expect(clockLines({ ...base, responseMet: true, slaPaused: true }, now)).toEqual([
			{ label: 'First response', text: 'Met', tone: 'success' },
			{ label: 'Resolution', text: 'Paused while waiting', tone: 'warning' }
		]);
		expect(clockLines({ ...base, responseMet: false, resolutionMet: true }, now)).toEqual([
			{ label: 'First response', text: 'Missed', tone: 'danger' },
			{ label: 'Resolution', text: 'Met', tone: 'success' }
		]);
	});

	it('knows when a target was missed', () => {
		expect(missed(base)).toBe(false);
		expect(missed({ ...base, responseBreachedAt: '2026-11-26T11:00:00Z' })).toBe(true);
		expect(missed({ ...base, resolutionMet: false })).toBe(true);
	});
});
