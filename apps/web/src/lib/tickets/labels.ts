import type { BadgeTone } from '@usehuxley/ui';

/** Priorities, most urgent first. */
export const PRIORITIES = [
	{ value: 'p1', label: 'P1 · Critical', short: 'P1', tone: 'danger' },
	{ value: 'p2', label: 'P2 · High', short: 'P2', tone: 'warning' },
	{ value: 'p3', label: 'P3 · Medium', short: 'P3', tone: 'info' },
	{ value: 'p4', label: 'P4 · Low', short: 'P4', tone: 'neutral' }
] as const satisfies readonly { value: string; label: string; short: string; tone: BadgeTone }[];

/** Impact and urgency: 1 (high) to 3 (low). */
export const LEVELS = [
	{ value: '1', label: 'High' },
	{ value: '2', label: 'Medium' },
	{ value: '3', label: 'Low' }
] as const;

export const SOURCES = [
	{ value: 'web', label: 'Web' },
	{ value: 'phone', label: 'Phone' },
	{ value: 'email', label: 'Email' },
	{ value: 'portal', label: 'Portal' },
	{ value: 'api', label: 'API' },
	{ value: 'workflow', label: 'Workflow' }
] as const;

export const NOTE_KINDS = [
	{ value: 'public', label: 'Reply', hint: 'The requester will see this.' },
	{ value: 'internal', label: 'Internal note', hint: 'Only your team sees this.' }
] as const;

/** The list's views: which tickets, by state and assignment. */
export const VIEWS = [
	{ value: 'mine', label: 'Assigned to me', query: { assignee: 'me', state: 'open' } },
	{ value: 'unassigned', label: 'Unassigned', query: { assignee: 'none', state: 'open' } },
	{ value: 'open', label: 'All open', query: { state: 'open' } },
	{ value: 'resolved', label: 'Resolved', query: { state: 'resolved' } },
	{ value: 'closed', label: 'Closed', query: { state: 'closed' } }
] as const;

export type View = (typeof VIEWS)[number]['value'];

export function priorityOf(value: string) {
	return PRIORITIES.find((p) => p.value === value) ?? PRIORITIES[2];
}

/** How a status category looks. */
export function categoryTone(category: string): BadgeTone {
	switch (category) {
		case 'new':
			return 'accent';
		case 'open':
			return 'info';
		case 'pending':
			return 'warning';
		case 'resolved':
			return 'success';
		default:
			return 'neutral';
	}
}

const units: [Intl.RelativeTimeFormatUnit, number][] = [
	['year', 365 * 24 * 3600],
	['month', 30 * 24 * 3600],
	['week', 7 * 24 * 3600],
	['day', 24 * 3600],
	['hour', 3600],
	['minute', 60]
];

/** "3 hours ago", "just now". */
export function ago(iso: string, now: Date = new Date()): string {
	const seconds = Math.round((new Date(iso).getTime() - now.getTime()) / 1000);
	const format = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
	for (const [unit, size] of units) {
		if (Math.abs(seconds) >= size) return format.format(Math.trunc(seconds / size), unit);
	}
	return 'just now';
}
