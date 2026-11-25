import type { components } from '@usehuxley/api-client';

type Map = components['schemas']['DependencyMap'];

export const NODE_WIDTH = 200;
const COLUMN = 280;
const ROW = 90;

/**
 * A left-to-right layout: what depends on something sits to its left, so
 * every arrow points at what's depended on. A host is to the right of
 * what it hosts. Assets in one column are stacked, centred on the asset
 * the map is of.
 */
export function layout(map: Map, centre: string) {
	// Each edge as dependent → dependency.
	const arrows = map.edges.map((e) =>
		e.kind === 'hosts'
			? { id: e.id, source: e.toId, target: e.fromId, label: 'hosted by' }
			: { id: e.id, source: e.fromId, target: e.toId, label: e.kind.replaceAll('_', ' ') }
	);
	const column = new Map<string, number>([[centre, 0]]);
	const queue = [centre];
	while (queue.length) {
		const id = queue.shift()!;
		const at = column.get(id)!;
		for (const a of arrows) {
			const next = a.source === id ? a.target : a.target === id ? a.source : null;
			if (next && !column.has(next)) {
				column.set(next, a.source === id ? at + 1 : at - 1);
				queue.push(next);
			}
		}
	}
	const byColumn = new Map<number, string[]>();
	for (const node of map.nodes) {
		const c = column.get(node.id) ?? 0;
		byColumn.set(c, [...(byColumn.get(c) ?? []), node.id]);
	}
	const position = new Map<string, { x: number; y: number }>();
	for (const [c, ids] of byColumn) {
		ids.forEach((id, i) =>
			position.set(id, { x: c * COLUMN, y: (i - (ids.length - 1) / 2) * ROW })
		);
	}
	return {
		nodes: map.nodes.map((n) => ({ ...n, position: position.get(n.id) ?? { x: 0, y: 0 } })),
		arrows
	};
}
