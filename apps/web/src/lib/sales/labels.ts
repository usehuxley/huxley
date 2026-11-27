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

export const QUOTE_STATUSES = [
	{ value: 'draft', label: 'Draft' },
	{ value: 'sent', label: 'Sent' },
	{ value: 'accepted', label: 'Accepted' },
	{ value: 'declined', label: 'Declined' }
] as const;

export function quoteTone(status: string): BadgeTone {
	return status === 'accepted'
		? 'success'
		: status === 'declined'
			? 'danger'
			: status === 'sent'
				? 'info'
				: 'neutral';
}

export const REVIEW_STATUSES = [
	{ value: 'not_needed', label: 'No review needed' },
	{ value: 'needed', label: 'Needs review' },
	{ value: 'in_review', label: 'In review' },
	{ value: 'approved', label: 'Approved' },
	{ value: 'changes_requested', label: 'Changes requested' },
	{ value: 'rejected', label: 'Rejected' }
] as const;

export function reviewTone(status: string): BadgeTone {
	return status === 'approved'
		? 'success'
		: status === 'rejected'
			? 'danger'
			: status === 'needed' || status === 'changes_requested'
				? 'warning'
				: status === 'in_review'
					? 'info'
					: 'neutral';
}

export const RULE_KINDS = [
	{ value: 'always', label: 'Every quote' },
	{ value: 'margin_below', label: 'Margin below (%)' },
	{ value: 'first_year_above', label: 'First year above (amount)' },
	{ value: 'discount_above', label: 'A line discounted more than (%)' },
	{ value: 'new_client', label: "The client's first quote" },
	{ value: 'category', label: 'Includes a product category' },
	{ value: 'expression', label: 'An expression is true' }
] as const;

export const ORDER_STATUSES = [
	{ value: 'open', label: 'Open' },
	{ value: 'fulfilled', label: 'Fulfilled' },
	{ value: 'closed', label: 'Closed' },
	{ value: 'cancelled', label: 'Cancelled' }
] as const;

export function orderTone(status: string): BadgeTone {
	return status === 'fulfilled' ? 'success' : status === 'open' ? 'info' : 'neutral';
}

/** `Q-42`. */
export const quoteRef = (n: number) => `Q-${n}`;

export const BILLINGS = [
	{ value: 'one_time', label: 'One-time' },
	{ value: 'monthly', label: 'Monthly' },
	{ value: 'quarterly', label: 'Quarterly' },
	{ value: 'annual', label: 'Annual' }
] as const;
