/** Money for people: `$1,000` (cents only when there are some). */
export function money(amount: string | number, currency = 'USD'): string {
	const n = typeof amount === 'number' ? amount : Number(amount);
	if (!Number.isFinite(n)) return String(amount);
	return new Intl.NumberFormat(undefined, {
		style: 'currency',
		currency,
		maximumFractionDigits: Number.isInteger(n) ? 0 : 2
	}).format(n);
}

export const BILLING: Record<string, string> = {
	one_time: 'one-time',
	monthly: 'per month',
	quarterly: 'per quarter',
	annual: 'per year'
};

export const STATUS: Record<
	string,
	{ label: string; tone: 'info' | 'success' | 'danger' | 'neutral' }
> = {
	sent: { label: 'Waiting for your answer', tone: 'info' },
	accepted: { label: 'Accepted', tone: 'success' },
	declined: { label: 'Declined', tone: 'danger' },
	draft: { label: 'Draft', tone: 'neutral' }
};
