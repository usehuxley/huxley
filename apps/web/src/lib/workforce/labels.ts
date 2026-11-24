import type { BadgeTone } from '@usehuxley/ui';

/** Skill levels, 1 to 4. */
export const LEVELS = [
	{ value: 1, label: 'Learning', short: 'L1' },
	{ value: 2, label: 'Working', short: 'L2' },
	{ value: 3, label: 'Proficient', short: 'L3' },
	{ value: 4, label: 'Expert', short: 'L4' }
] as const;

export function levelLabel(level: number): string {
	return LEVELS.find((l) => l.value === level)?.label ?? `Level ${level}`;
}

/** How a held certification's state looks. */
export function certTone(state: string): BadgeTone {
	return state === 'expired' ? 'danger' : state === 'expiring' ? 'warning' : 'success';
}

export function certState(state: string): string {
	return state === 'expired' ? 'Expired' : state === 'expiring' ? 'Expiring soon' : 'Valid';
}

/** Skills grouped by category, categories and skills in order. */
export function byCategory<T extends { category: string; name: string }>(
	skills: readonly T[]
): [string, T[]][] {
	const groups = new Map<string, T[]>();
	for (const s of skills) {
		const key = s.category || 'Other';
		groups.set(key, [...(groups.get(key) ?? []), s]);
	}
	return [...groups.entries()].sort(([a], [b]) =>
		a === 'Other' ? 1 : b === 'Other' ? -1 : a.localeCompare(b)
	);
}
