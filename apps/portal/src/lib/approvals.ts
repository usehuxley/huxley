import type { BadgeTone } from '@usehuxley/ui';

/** An approval's status as a label. */
export const APPROVAL_STATUS: Record<string, { label: string; tone: BadgeTone }> = {
	pending: { label: 'Waiting for you', tone: 'warning' },
	approved: { label: 'Approved', tone: 'success' },
	rejected: { label: 'Rejected', tone: 'danger' },
	expired: { label: 'Expired', tone: 'neutral' }
};
