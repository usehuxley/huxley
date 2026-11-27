import type { BadgeTone } from '@usehuxley/ui';

export const PO_STATUSES = [
	{ value: 'draft', label: 'Draft' },
	{ value: 'submitted', label: 'Waiting for approval' },
	{ value: 'approved', label: 'Approved' },
	{ value: 'ordered', label: 'Ordered' },
	{ value: 'partially_received', label: 'Partly received' },
	{ value: 'received', label: 'Received' },
	{ value: 'cancelled', label: 'Cancelled' }
] as const;

export function poTone(status: string): BadgeTone {
	switch (status) {
		case 'submitted':
			return 'warning';
		case 'approved':
		case 'ordered':
		case 'partially_received':
			return 'info';
		case 'received':
			return 'success';
		default:
			return 'neutral';
	}
}

/** `PO-12`. */
export const poRef = (n: number) => `PO-${n}`;

export function money(amount: string | number, currency = 'USD'): string {
	return Number(amount).toLocaleString(undefined, { style: 'currency', currency });
}
