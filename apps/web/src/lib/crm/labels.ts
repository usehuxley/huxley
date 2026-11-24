import type { BadgeTone } from '@usehuxley/ui';

/** Company relationships, as the API names them, with labels. */
export const RELATIONSHIPS = [
	{ value: 'client', label: 'Client' },
	{ value: 'prospect', label: 'Prospect' },
	{ value: 'vendor', label: 'Vendor' },
	{ value: 'partner', label: 'Partner' }
];

export const STATUSES = [
	{ value: 'active', label: 'Active' },
	{ value: 'onboarding', label: 'Onboarding' },
	{ value: 'offboarding', label: 'Offboarding' },
	{ value: 'inactive', label: 'Inactive' }
];

export const label = (list: { value: string; label: string }[], value: string) =>
	list.find((o) => o.value === value)?.label ?? value;

export function statusTone(status: string): BadgeTone {
	switch (status) {
		case 'active':
			return 'success';
		case 'onboarding':
			return 'accent';
		case 'offboarding':
			return 'warning';
		default:
			return 'neutral';
	}
}

/** Weekdays, as the API keys opening hours. */
export const DAYS = [
	{ key: 'mon', label: 'Monday' },
	{ key: 'tue', label: 'Tuesday' },
	{ key: 'wed', label: 'Wednesday' },
	{ key: 'thu', label: 'Thursday' },
	{ key: 'fri', label: 'Friday' },
	{ key: 'sat', label: 'Saturday' },
	{ key: 'sun', label: 'Sunday' }
] as const;

/** Opening hours as one line, e.g. "Mon–Fri 09:00–17:00". */
export function hoursSummary(hours: Record<string, string[][]> | null | undefined): string {
	if (!hours || Object.keys(hours).length === 0) return 'No opening hours';
	const text = (day: string) => (hours[day] ?? []).map(([a, b]) => `${a}–${b}`).join(', ');
	const parts: string[] = [];
	let start = 0;
	for (let i = 1; i <= DAYS.length; i++) {
		const same = i < DAYS.length && text(DAYS[i].key) === text(DAYS[start].key);
		if (same) continue;
		const value = text(DAYS[start].key);
		if (value) {
			const first = DAYS[start].label.slice(0, 3);
			const last = DAYS[i - 1].label.slice(0, 3);
			parts.push(`${first === last ? first : `${first}–${last}`} ${value}`);
		}
		start = i;
	}
	return parts.join('; ');
}
