import type { BadgeTone } from '@usehuxley/ui';

/** Why a ticket needs attention, as the API says it, for people. */
export const ATTENTION: Record<string, { label: string; tone: BadgeTone }> = {
	breached: { label: 'SLA missed', tone: 'danger' },
	replied: { label: 'Requester replied', tone: 'info' },
	due_soon: { label: 'Due soon', tone: 'warning' },
	new: { label: 'New', tone: 'neutral' }
};

/** When a ticket's next SLA target is due: first response, then resolution. */
export function nextDue(t: {
	firstResponseAt?: string | null;
	responseDueAt?: string | null;
	resolutionDueAt?: string | null;
}): string | null {
	if (!t.firstResponseAt && t.responseDueAt) return t.responseDueAt;
	return t.resolutionDueAt ?? null;
}
