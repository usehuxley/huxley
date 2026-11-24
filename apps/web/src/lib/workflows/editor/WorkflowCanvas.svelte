<script lang="ts">
	import {
		Background,
		Controls,
		MiniMap,
		Panel,
		SvelteFlow,
		type Edge,
		type Node
	} from '@xyflow/svelte';
	import { ArrowDown, ArrowRight } from '@lucide/svelte';
	import '@xyflow/svelte/dist/style.css';
	import AddEdge from './AddEdge.svelte';
	import MarkerNode from './MarkerNode.svelte';
	import Follow from './Follow.svelte';
	import Opening from './Opening.svelte';
	import StepNode from './StepNode.svelte';
	import {
		MARKER,
		NODE_HEIGHT,
		NODE_WIDTH,
		layout,
		type Direction,
		type Insert
	} from './layout.ts';
	import { pathKey } from './catalog.ts';
	import type { EditorContext } from './context.ts';

	let {
		editor,
		picking,
		onpick
	}: {
		editor: EditorContext;
		/** The "+" whose step picker is open. */
		picking: Insert | null;
		onpick: (insert: Insert) => void;
	} = $props();

	const nodeTypes = { step: StepNode, marker: MarkerNode };
	const edgeTypes = { add: AddEdge };

	// Top to bottom or left to right: this browser's choice, not part of
	// the workflow.
	const DIRECTION_KEY = 'huxley.canvas.direction';
	function savedDirection(): Direction {
		try {
			return localStorage.getItem(DIRECTION_KEY) === 'right' ? 'right' : 'down';
		} catch {
			return 'down';
		}
	}
	let direction = $state<Direction>(savedDirection());
	function setDirection(next: Direction) {
		direction = next;
		try {
			localStorage.setItem(DIRECTION_KEY, next);
		} catch {
			// Private browsing and the like: the choice lasts for this page.
		}
	}
	const DIRECTIONS: { value: Direction; label: string; Icon: typeof ArrowDown }[] = [
		{ value: 'down', label: 'Top to bottom', Icon: ArrowDown },
		{ value: 'right', label: 'Left to right', Icon: ArrowRight }
	];

	let selectedKey = $derived(editor.selected ? pathKey(editor.selected) : null);
	let placed = $derived(layout(editor.document, selectedKey, editor.readonly, direction));

	let width = $state(0);
	let height = $state(0);
	let container = $state<HTMLElement>();
	let selectedAt = $derived.by(() => {
		const node = placed.nodes.find((n) => n.id === selectedKey);
		return node ? { x: node.x, y: node.y } : null;
	});
	// The workflow's extent, for the opening view.
	let bounds = $derived.by(() => {
		const big = (n: (typeof placed.nodes)[number]) => n.kind === 'step' || n.kind === 'start';
		return {
			left: Math.min(...placed.nodes.map((n) => n.x)),
			top: Math.min(...placed.nodes.map((n) => n.y)),
			right: Math.max(...placed.nodes.map((n) => n.x + (big(n) ? NODE_WIDTH : MARKER))),
			bottom: Math.max(...placed.nodes.map((n) => n.y + (big(n) ? NODE_HEIGHT : MARKER)))
		};
	});

	const same = (a: Insert | undefined, b: Insert | null) =>
		!!a && !!b && a.index === b.index && a.list.join('.') === b.list.join('.');

	function toNodes(): Node[] {
		return placed.nodes.map((n) => {
			const common = {
				id: n.id,
				position: { x: n.x, y: n.y },
				draggable: false,
				connectable: false,
				selectable: false
			};
			if (n.kind === 'step') {
				return {
					...common,
					type: 'step',
					width: NODE_WIDTH,
					height: NODE_HEIGHT,
					data: { step: n.step, path: n.path, editor, direction }
				};
			}
			const big = n.kind === 'start';
			return {
				...common,
				type: 'marker',
				width: big ? NODE_WIDTH : MARKER,
				height: big ? NODE_HEIGHT : MARKER,
				data:
					n.kind === 'start'
						? {
								kind: 'start',
								title: n.title,
								detail: n.detail,
								failure: n.id === 'on_error',
								direction
							}
						: { kind: n.kind, direction }
			};
		});
	}

	function toEdges(): Edge[] {
		return placed.edges.map((e) => ({
			id: e.id,
			source: e.source,
			target: e.target,
			type: 'add',
			selectable: false,
			data: {
				label: e.label,
				insert: e.insert,
				active: same(e.insert, picking),
				direction,
				onpick
			}
		}));
	}

	// Filled at once, so the first fit-to-view sees the workflow.
	let nodes = $state.raw<Node[]>(toNodes());
	let edges = $state.raw<Edge[]>(toEdges());
	$effect(() => {
		nodes = toNodes();
		edges = toEdges();
	});
</script>

<div
	bind:this={container}
	bind:clientWidth={width}
	bind:clientHeight={height}
	class="workflow-canvas h-[70vh] min-h-[28rem] overflow-hidden rounded-card border border-border"
	role="group"
	aria-label="Workflow canvas"
>
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		{edgeTypes}
		minZoom={0.2}
		maxZoom={1.5}
		nodesDraggable={false}
		nodesConnectable={false}
		elementsSelectable={false}
		nodesFocusable={false}
		edgesFocusable={false}
		panOnScroll
		zoomOnDoubleClick={false}
		proOptions={{ hideAttribution: false }}
	>
		<Background gap={20} />
		<Controls showLock={false} />
		<MiniMap pannable zoomable />
		<Opening {width} {height} {bounds} {direction} />
		<Follow selected={selectedAt} {width} {height} {container} {direction} />
		<Panel position="top-right">
			<div
				class="flex overflow-hidden rounded-control border border-border bg-surface shadow-sm"
				role="radiogroup"
				aria-label="Layout direction"
			>
				{#each DIRECTIONS as option (option.value)}
					<button
						type="button"
						role="radio"
						aria-checked={direction === option.value}
						aria-label={option.label}
						title={option.label}
						class="flex size-8 items-center justify-center {direction === option.value
							? 'bg-accent text-white'
							: 'text-fg-muted hover:bg-surface-sunken hover:text-fg'}"
						onclick={() => setDirection(option.value)}
					>
						<option.Icon size={16} />
					</button>
				{/each}
			</div>
		</Panel>
	</SvelteFlow>
</div>

<style>
	/* The canvas follows Huxley's theme (light, dark, or the OS's). */
	.workflow-canvas :global(.svelte-flow) {
		--xy-background-color: var(--hx-surface-sunken);
		--xy-background-pattern-color: var(--hx-border);
		--xy-edge-stroke: var(--hx-border);
		--xy-controls-button-background-color: var(--hx-surface);
		--xy-controls-button-background-color-hover: var(--hx-surface-raised);
		--xy-controls-button-color: var(--hx-fg);
		--xy-controls-button-border-color: var(--hx-border);
		--xy-minimap-background-color: var(--hx-surface);
		--xy-minimap-mask-background-color: color-mix(
			in srgb,
			var(--hx-surface-sunken) 70%,
			transparent
		);
		--xy-node-background-color: transparent;
		--xy-node-border: none;
		--xy-node-boxshadow-hover: none;
		--xy-node-boxshadow-selected: none;
		--xy-attribution-background-color: transparent;
		--xy-edge-label-background-color: transparent;
	}
	.workflow-canvas :global(.svelte-flow__node) {
		padding: 0;
		border: none;
		background: transparent;
		width: auto;
	}
</style>
