import type { BadgeTone } from '@usehuxley/ui';

export const STATUSES = [
	{ value: 'planned', label: 'Planned' },
	{ value: 'active', label: 'Active' },
	{ value: 'on_hold', label: 'On hold' },
	{ value: 'completed', label: 'Completed' },
	{ value: 'cancelled', label: 'Cancelled' }
] as const;

export const BILLING = [
	{ value: 'time_materials', label: 'Time and materials' },
	{ value: 'fixed_fee', label: 'Fixed fee' },
	{ value: 'milestone', label: 'Fixed fee, by milestone' },
	{ value: 'not_to_exceed', label: 'Not to exceed' },
	{ value: 'agreement', label: 'Covered by an agreement' }
] as const;

export const HEALTH: Record<string, [string, BadgeTone]> = {
	on_track: ['On track', 'success'],
	at_risk: ['At risk', 'warning'],
	off_track: ['Off track', 'danger']
};

export const label = (list: readonly { value: string; label: string }[], value: string) =>
	list.find((x) => x.value === value)?.label ?? value;
