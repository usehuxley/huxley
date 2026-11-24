import { LEVELS, priorityOf } from './labels';

type Change = { from?: unknown; to?: unknown; changed?: boolean };

const FIELDS: Record<string, string> = {
	title: 'title',
	description: 'description',
	impact: 'impact',
	urgency: 'urgency',
	priority: 'priority',
	queue: 'queue',
	requester: 'requester',
	assignee: 'assignee',
	team: 'team'
};

/** A value from an activity entry as text. */
function text(value: unknown): string {
	return typeof value === 'string' || typeof value === 'number'
		? String(value)
		: JSON.stringify(value);
}

function shown(field: string, value: unknown): string {
	if (value === null || value === undefined || value === '') return 'none';
	if (field === 'impact' || field === 'urgency') {
		return LEVELS.find((l) => l.value === text(value))?.label.toLowerCase() ?? text(value);
	}
	if (field === 'priority') return priorityOf(text(value)).short;
	return text(value);
}

/**
 * One line describing an activity entry, e.g. "changed the status from New
 * to In progress". `statusName` turns a status key into its name.
 */
export function describe(
	kind: string,
	details: Record<string, unknown>,
	statusName: (key: string) => string = (k) => k
): string {
	switch (kind) {
		case 'created':
			return 'created the ticket';
		case 'status':
			return `changed the status from ${statusName(text(details.from))} to ${statusName(text(details.to))}`;
		case 'note':
			return details.kind === 'internal'
				? 'added an internal note'
				: details.kind === 'resolution'
					? 'added a resolution'
					: 'replied';
		default: {
			const parts = Object.entries(details as Record<string, Change>)
				.filter(([field]) => field in FIELDS)
				.map(([field, change]) => {
					if (change.changed) return `edited the ${FIELDS[field]}`;
					if (field === 'assignee' || field === 'team') {
						return change.to === null || change.to === undefined
							? `unassigned ${field === 'team' ? 'the team' : shown(field, change.from)}`
							: `assigned ${field === 'team' ? 'team ' : ''}${shown(field, change.to)}`;
					}
					return `changed the ${FIELDS[field]} from ${shown(field, change.from)} to ${shown(field, change.to)}`;
				});
			return parts.length > 0 ? parts.join(', ') : 'updated the ticket';
		}
	}
}
