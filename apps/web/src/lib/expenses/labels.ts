import type { BadgeTone } from '@usehuxley/ui';

export const EXPENSE_STATUSES = [
	{ value: 'draft', label: 'Draft' },
	{ value: 'submitted', label: 'Submitted' },
	{ value: 'approved', label: 'Approved' },
	{ value: 'rejected', label: 'Sent back' },
	{ value: 'reimbursed', label: 'Reimbursed' }
] as const;

export function expenseTone(status: string): BadgeTone {
	return status === 'approved' || status === 'reimbursed'
		? 'success'
		: status === 'rejected'
			? 'warning'
			: status === 'submitted'
				? 'info'
				: 'neutral';
}

export const FLAGS: Record<string, string> = {
	over_limit: 'Over the limit',
	no_receipt: 'No receipt'
};

export function money(amount: string | number, currency = 'USD'): string {
	return Number(amount).toLocaleString(undefined, { style: 'currency', currency });
}
