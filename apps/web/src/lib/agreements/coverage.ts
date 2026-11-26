import type { BadgeTone } from '@usehuxley/ui';
import type { components } from '@usehuxley/api-client';

export type Outcome = components['schemas']['CoverageOutcome'];
export type AgreementKind = components['schemas']['AgreementKind'];

/** How each outcome reads, and its badge. */
export const OUTCOME: Record<Outcome, { label: string; tone: BadgeTone }> = {
	covered: { label: 'Covered', tone: 'success' },
	partly_covered: { label: 'Partly covered', tone: 'warning' },
	overage: { label: 'Overage', tone: 'warning' },
	billable: { label: 'Billable', tone: 'info' },
	non_billable: { label: 'Not billable', tone: 'neutral' },
	no_agreement: { label: 'No agreement', tone: 'danger' }
};

export const KINDS: { value: AgreementKind; label: string }[] = [
	{ value: 'managed', label: 'Managed services' },
	{ value: 'block_hours', label: 'Block hours' },
	{ value: 'time_materials', label: 'Time and materials' },
	{ value: 'project', label: 'Project' }
];

export const kindLabel = (k: AgreementKind) => KINDS.find((x) => x.value === k)?.label ?? k;

/** Minutes as hours, e.g. 90 → "1.5 h". */
export function hours(minutes: number): string {
	const h = Math.round((minutes / 60) * 100) / 100;
	return `${h} h`;
}
