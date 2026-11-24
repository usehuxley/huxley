import { describe, expect, it } from 'vitest';
import { GAP_X, NODE_HEIGHT, NODE_WIDTH, layout, type CanvasNode } from './layout.ts';

const doc = (steps: unknown[], extra: Record<string, unknown> = {}) => ({
	spec: { triggers: [{ type: 'manual' }], steps, ...extra }
});

const edge = (l: ReturnType<typeof layout>, source: string, target: string) =>
	l.edges.find((e) => e.source === source && e.target === target);

/** Nodes' boxes (markers are smaller, but a full box is a safe bound). */
function overlaps(nodes: CanvasNode[]): string[] {
	const boxes = nodes.filter((n) => n.kind === 'step');
	const found: string[] = [];
	for (const [i, a] of boxes.entries()) {
		for (const b of boxes.slice(i + 1)) {
			const apart =
				a.x + NODE_WIDTH <= b.x ||
				b.x + NODE_WIDTH <= a.x ||
				a.y + NODE_HEIGHT <= b.y ||
				b.y + NODE_HEIGHT <= a.y;
			if (!apart) found.push(`${a.id} / ${b.id}`);
		}
	}
	return found;
}

describe('canvas layout', () => {
	it('lays out a straight line with a "+" on every edge', () => {
		const l = layout(
			doc([
				{ id: 'greet', type: 'transform' },
				{ id: 'send', type: 'http.request' }
			]),
			null
		);
		expect(l.nodes.map((n) => n.id)).toEqual([
			'start',
			'spec.steps[0]',
			'spec.steps[1]',
			'end',
			'on_error',
			'on_error:end'
		]);
		expect(edge(l, 'start', 'spec.steps[0]')?.insert).toEqual({
			list: ['spec', 'steps'],
			index: 0,
			label: 'Add a step at the start of Steps'
		});
		expect(edge(l, 'spec.steps[0]', 'spec.steps[1]')?.insert?.label).toBe('Add a step after greet');
		expect(edge(l, 'spec.steps[1]', 'end')?.insert).toEqual({
			list: ['spec', 'steps'],
			index: 2,
			label: 'Add a step after send'
		});
		// Top to bottom, one column.
		const [start, a, b] = l.nodes;
		expect(a.y).toBeGreaterThan(start.y);
		expect(b.y).toBeGreaterThan(a.y);
		expect(a.x).toBe(b.x);
		expect(l.nodes.find((n) => n.id === 'start')).toMatchObject({ detail: 'manual' });
	});

	it('fans an if out into labelled lanes that rejoin', () => {
		const l = layout(
			doc([
				{
					id: 'check',
					type: 'if',
					condition: '{{ x }}',
					then: [{ id: 'yes', type: 'transform' }],
					else: []
				},
				{ id: 'after', type: 'transform' }
			]),
			null
		);
		const ifId = 'spec.steps[0]';
		const then = edge(l, ifId, 'spec.steps[0].then[0]');
		expect(then?.label).toBe('Then');
		expect(then?.insert?.label).toBe('Add a step at the start of Then');
		// The empty else has a placeholder of its own: its label and "+" go
		// on the edge to it, and it leads to the join.
		const otherwise = edge(l, ifId, `${ifId}.else:empty`);
		expect(otherwise?.label).toBe('Else');
		expect(otherwise?.insert).toEqual({
			list: ['spec', 'steps', 0, 'else'],
			index: 0,
			label: 'Add a step at the start of Else'
		});
		expect(edge(l, `${ifId}.else:empty`, `${ifId}:join`)?.insert).toBeUndefined();
		expect(edge(l, 'spec.steps[0].then[0]', `${ifId}:join`)?.insert?.label).toBe(
			'Add a step after yes'
		);
		// After the join comes the next step: inserting there goes after the if.
		expect(edge(l, `${ifId}:join`, 'spec.steps[1]')?.insert).toEqual({
			list: ['spec', 'steps'],
			index: 1,
			label: 'Add a step after check'
		});
		const yes = l.nodes.find((n) => n.id === 'spec.steps[0].then[0]')!;
		const after = l.nodes.find((n) => n.id === 'spec.steps[1]')!;
		expect(after.y).toBeGreaterThan(yes.y);
	});

	it('keeps nested lanes side by side without overlapping', () => {
		const inner = {
			id: 'inner',
			type: 'if',
			condition: '{{ y }}',
			then: [{ id: 'a', type: 'transform' }],
			else: [{ id: 'b', type: 'transform' }]
		};
		const l = layout(
			doc([
				{
					id: 'route',
					type: 'switch',
					value: '{{ v }}',
					cases: [
						{ when: 'x', steps: [inner] },
						{ when: 'y', steps: [{ id: 'c', type: 'transform' }] }
					],
					default: [{ id: 'd', type: 'transform' }]
				},
				{
					id: 'both',
					type: 'parallel',
					branches: [
						{ id: 'left', steps: [{ id: 'e', type: 'transform' }] },
						{ id: 'right', steps: [] }
					]
				}
			]),
			null
		);
		expect(overlaps(l.nodes)).toEqual([]);
		expect(edge(l, 'spec.steps[0]', 'spec.steps[0].cases[0].steps[0]')?.label).toBe('When x');
		expect(edge(l, 'spec.steps[0]', 'spec.steps[0].default[0]')?.label).toBe('Otherwise');
		expect(edge(l, 'spec.steps[1]', 'spec.steps[1].branches[0].steps[0]')?.label).toBe(
			'Branch left'
		);
		// Lanes sit at least a gap apart.
		const a = l.nodes.find((n) => n.id.endsWith('.then[0]'))!;
		const b = l.nodes.find((n) => n.id.endsWith('.else[0]'))!;
		expect(Math.abs(a.x - b.x)).toBeGreaterThanOrEqual(NODE_WIDTH + GAP_X);
	});

	it('shows optional blocks once they have steps or their step is selected', () => {
		const tryStep = { id: 't', type: 'try', steps: [], catch: [] };
		const labels = (selected: string | null) =>
			layout(doc([tryStep]), selected).edges.flatMap((e) => (e.label ? [e.label] : []));
		expect(labels(null)).toEqual(['Try', 'If it fails']);
		expect(labels('spec.steps[0]')).toEqual(['Try', 'If it fails', 'Afterwards']);
	});

	it('an empty workflow still offers a first step, and read-only offers none', () => {
		const empty = layout(doc([]), null);
		expect(edge(empty, 'start', 'end')?.insert?.label).toBe('Add a step at the start of Steps');
		expect(edge(empty, 'on_error', 'on_error:end')?.insert?.label).toBe(
			'Add a step at the start of If the run fails'
		);
		const readonly = layout(doc([{ id: 'greet', type: 'transform' }]), null, true);
		expect(readonly.edges.every((e) => e.insert === undefined)).toBe(true);
	});

	it('gives empty lanes their own place, so their "+" never coincide', () => {
		const l = layout(
			doc([{ id: 'c', type: 'if', condition: '{{ x }}', then: [], else: [] }]),
			null
		);
		const empties = l.nodes.filter((n) => n.kind === 'empty');
		expect(empties.map((n) => n.id)).toEqual([
			'spec.steps[0].then:empty',
			'spec.steps[0].else:empty'
		]);
		expect(empties[0].x).not.toBe(empties[1].x);
	});

	it('gives every edge a distinct id', () => {
		const l = layout(
			doc([
				{
					id: 'p',
					type: 'parallel',
					branches: [
						{ id: 'a', steps: [] },
						{ id: 'b', steps: [] }
					]
				}
			]),
			null
		);
		const ids = l.edges.map((e) => e.id);
		expect(new Set(ids).size).toBe(ids.length);
	});

	it('lays out left to right: steps go right, lanes stack, failures go below', () => {
		const l = layout(
			doc(
				[
					{ id: 'first', type: 'transform' },
					{
						id: 'check',
						type: 'if',
						condition: '{{ x }}',
						then: [{ id: 'yes', type: 'transform' }],
						else: [{ id: 'no', type: 'transform' }]
					},
					{ id: 'last', type: 'transform' }
				],
				{ on_error: [{ id: 'alert', type: 'core.notify' }] }
			),
			null,
			false,
			'right'
		);
		const at = (id: string) => l.nodes.find((n) => n.id === id)!;
		const [start, first, check, last] = [
			'start',
			'spec.steps[0]',
			'spec.steps[1]',
			'spec.steps[2]'
		].map(at);
		// One row, left to right.
		expect(first.x).toBeGreaterThan(start.x);
		expect(check.x).toBeGreaterThan(first.x);
		expect(last.x).toBeGreaterThan(check.x);
		expect(first.y).toBe(check.y);
		// Then and Else stack vertically, right of the If, in the same column.
		const yes = at('spec.steps[1].then[0]');
		const no = at('spec.steps[1].else[0]');
		expect(yes.x).toBe(no.x);
		expect(yes.x).toBeGreaterThan(check.x);
		expect(Math.abs(yes.y - no.y)).toBeGreaterThanOrEqual(NODE_HEIGHT);
		// The failure flow sits below the main one.
		const failure = at('on_error');
		const lowest = Math.max(
			...l.nodes
				.filter((n) => n.kind === 'step' && !n.id.startsWith('spec.on_error'))
				.map((n) => n.y)
		);
		expect(failure.y).toBeGreaterThan(lowest);
		// Nothing overlaps (in this direction, a node is wide along the flow).
		const steps = l.nodes.filter((n) => n.kind === 'step');
		for (const [i, a] of steps.entries()) {
			for (const b of steps.slice(i + 1)) {
				const apart =
					a.x + NODE_WIDTH <= b.x ||
					b.x + NODE_WIDTH <= a.x ||
					a.y + NODE_HEIGHT <= b.y ||
					b.y + NODE_HEIGHT <= a.y;
				expect(apart, `${a.id} / ${b.id}`).toBe(true);
			}
		}
		// The same edges and "+" as top to bottom.
		const down = layout(doc([{ id: 'first', type: 'transform' }]), null, false, 'down').edges.map(
			(e) => [e.source, e.target, e.insert?.label]
		);
		const right = layout(doc([{ id: 'first', type: 'transform' }]), null, false, 'right').edges.map(
			(e) => [e.source, e.target, e.insert?.label]
		);
		expect(right).toEqual(down);
	});
});
