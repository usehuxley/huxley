import type { BadgeTone } from '@usehuxley/ui';
import { ago } from './labels';

/** The SLA fields of a ticket. */
export type SlaFields = {
	slaPolicy?: string | null;
	responseDueAt?: string | null;
	responseMet?: boolean | null;
	responseBreachedAt?: string | null;
	resolutionDueAt?: string | null;
	resolutionMet?: boolean | null;
	resolutionBreachedAt?: string | null;
	slaPaused: boolean;
	firstResponseAt?: string | null;
};

export type ClockLine = { label: string; text: string; tone: BadgeTone };

function due(at: string, now: Date): { text: string; tone: BadgeTone } {
	const overdue = new Date(at).getTime() <= now.getTime();
	return overdue
		? { text: `Overdue (due ${ago(at, now)})`, tone: 'danger' }
		: { text: `Due ${ago(at, now)}`, tone: 'neutral' };
}

/** How each SLA clock stands, for the ticket page. Empty without an SLA. */
export function clockLines(t: SlaFields, now: Date = new Date()): ClockLine[] {
	if (!t.slaPolicy) return [];
	const lines: ClockLine[] = [];
	if (t.responseMet === true) {
		lines.push({ label: 'First response', text: 'Met', tone: 'success' });
	} else if (t.responseMet === false || (t.responseBreachedAt && t.firstResponseAt)) {
		lines.push({ label: 'First response', text: 'Missed', tone: 'danger' });
	} else if (t.responseDueAt) {
		lines.push({ label: 'First response', ...due(t.responseDueAt, now) });
	}
	if (t.resolutionMet === true) {
		lines.push({ label: 'Resolution', text: 'Met', tone: 'success' });
	} else if (t.resolutionMet === false) {
		lines.push({ label: 'Resolution', text: 'Missed', tone: 'danger' });
	} else if (t.slaPaused) {
		lines.push({ label: 'Resolution', text: 'Paused while waiting', tone: 'warning' });
	} else if (t.resolutionDueAt) {
		lines.push({ label: 'Resolution', ...due(t.resolutionDueAt, now) });
	}
	return lines;
}

/** Whether any SLA target was missed. */
export function missed(t: SlaFields): boolean {
	return Boolean(
		t.responseBreachedAt ||
		t.resolutionBreachedAt ||
		t.responseMet === false ||
		t.resolutionMet === false
	);
}
