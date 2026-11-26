import type { BadgeTone } from '@usehuxley/ui';

/** Timesheet statuses and history actions: how they read. */
export const STATUS: Record<string, { label: string; tone: BadgeTone }> = {
	open: { label: 'Open', tone: 'neutral' },
	submitted: { label: 'Submitted', tone: 'info' },
	approved: { label: 'Approved', tone: 'success' },
	rejected: { label: 'Rejected', tone: 'danger' },
	reopened: { label: 'Reopened', tone: 'warning' },
	withdrawn: { label: 'Withdrawn', tone: 'neutral' }
};
