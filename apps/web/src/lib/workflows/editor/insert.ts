import { isStep, newStep, type Path } from './catalog.ts';
import type { Insert } from './layout.ts';

/** The list at `path` in `document`, created if it isn't there yet. */
export function listAt(document: Record<string, unknown>, path: Path): unknown[] {
	let owner: Record<string | number, unknown> = document;
	for (const part of path.slice(0, -1)) {
		const next = owner[part];
		if (!isStep(next) && !Array.isArray(next)) owner[part] = {};
		owner = owner[part] as Record<string | number, unknown>;
	}
	const field = path[path.length - 1];
	if (!Array.isArray(owner[field])) owner[field] = [];
	return owner[field] as unknown[];
}

/** Inserts a new step of `type` where `insert` says; returns its path. */
export function insertStep(document: Record<string, unknown>, insert: Insert, type: string): Path {
	const step = newStep(type, document);
	listAt(document, insert.list).splice(insert.index, 0, step);
	return [...insert.list, insert.index];
}
