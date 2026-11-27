import type { BadgeTone } from '@usehuxley/ui';

/** Money for people: `$14,820` (no cents when there are none). */
export function money(amount: string | number, currency = 'USD'): string {
	const n = typeof amount === 'number' ? amount : Number(amount);
	if (!Number.isFinite(n)) return String(amount);
	return new Intl.NumberFormat(undefined, {
		style: 'currency',
		currency,
		maximumFractionDigits: Number.isInteger(n) ? 0 : 2
	}).format(n);
}

export const FORECASTS = [
	{ value: 'pipeline', label: 'Pipeline' },
	{ value: 'best_case', label: 'Best case' },
	{ value: 'commit', label: 'Commit' },
	{ value: 'omitted', label: 'Omitted' }
] as const;

export const LEAD_STATUSES = [
	{ value: 'new', label: 'New' },
	{ value: 'working', label: 'Working' },
	{ value: 'disqualified', label: 'Disqualified' },
	{ value: 'converted', label: 'Converted' }
] as const;

export function leadTone(status: string): BadgeTone {
	return status === 'new'
		? 'accent'
		: status === 'working'
			? 'info'
			: status === 'converted'
				? 'success'
				: 'neutral';
}

export function statusTone(status: string): BadgeTone {
	return status === 'won' ? 'success' : status === 'lost' ? 'danger' : 'info';
}

export const label = (list: readonly { value: string; label: string }[], value: string) =>
	list.find((x) => x.value === value)?.label ?? value;

/** `OPP-42`. */
export const oppRef = (n: number) => `OPP-${n}`;
