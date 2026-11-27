import type { BadgeTone } from '@usehuxley/ui';

export const INVOICE_STATUSES = [
	{ value: 'draft', label: 'Draft' },
	{ value: 'approved', label: 'Approved' },
	{ value: 'sent', label: 'Sent' },
	{ value: 'void', label: 'Void' }
] as const;

export function invoiceTone(status: string): BadgeTone {
	return status === 'approved' ? 'info' : status === 'sent' ? 'success' : 'neutral';
}

export const invRef = (n: number) => `INV-${n}`;

export function money(amount: string | number, currency = 'USD'): string {
	return Number(amount).toLocaleString(undefined, { style: 'currency', currency });
}

/** The previous calendar month, as YYYY-MM-DD bounds. */
export function lastMonth(): { from: string; to: string } {
	const now = new Date(Date.now());
	const first = Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - 1, 1);
	const last = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 0);
	const day = (t: number) => new Date(t).toISOString().slice(0, 10);
	return { from: day(first), to: day(last) };
}
