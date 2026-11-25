<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import {
		Background,
		Controls,
		MarkerType,
		Position,
		SvelteFlow,
		type Edge,
		type Node
	} from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import type { components } from '@usehuxley/api-client';
	import { NODE_WIDTH, layout } from './map.ts';

	let { map, centre }: { map: components['schemas']['DependencyMap']; centre: string } = $props();

	const tone = (status: string) =>
		status === 'retired' || status === 'disposed'
			? 'opacity-60'
			: status === 'maintenance'
				? 'border-warning'
				: '';

	let laid = $derived(layout(map, centre));
	let nodes: Node[] = $state([]);
	let edges: Edge[] = $state([]);
	$effect(() => {
		nodes = laid.nodes.map((n) => ({
			id: n.id,
			position: n.position,
			data: { label: `${n.name}\n${n.typeName}` },
			sourcePosition: Position.Right,
			targetPosition: Position.Left,
			width: NODE_WIDTH,
			class: `map-node ${n.id === centre ? 'map-centre' : ''} ${tone(n.status)}`,
			ariaLabel: `${n.name}, ${n.typeName}`
		}));
		edges = laid.arrows.map((a) => ({
			id: a.id,
			source: a.source,
			target: a.target,
			label: a.label,
			markerEnd: { type: MarkerType.ArrowClosed },
			class: 'map-edge'
		}));
	});
</script>

<div
	class="h-[26rem] w-full overflow-hidden rounded-control border border-border"
	data-testid="dependency-map"
>
	<SvelteFlow
		bind:nodes
		bind:edges
		fitView
		minZoom={0.2}
		maxZoom={1.5}
		nodesDraggable={false}
		nodesConnectable={false}
		elementsSelectable={false}
		onnodeclick={({ node }) => {
			if (node.id !== centre) void goto(resolve('/(app)/assets/[id]', { id: node.id }));
		}}
	>
		<Background />
		<Controls showLock={false} />
	</SvelteFlow>
</div>

<style>
	:global(.map-node) {
		white-space: pre-line;
		font-size: 0.75rem;
		background: var(--color-surface);
		color: var(--color-fg);
		border-color: var(--color-border-strong);
		cursor: pointer;
	}
	:global(.map-centre) {
		border-color: var(--color-accent);
		border-width: 2px;
		font-weight: 600;
		cursor: default;
	}
</style>
