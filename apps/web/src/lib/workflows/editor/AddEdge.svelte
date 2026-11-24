<script lang="ts">
	import { BaseEdge, EdgeLabel, getSmoothStepPath, type EdgeProps } from '@xyflow/svelte';
	import { Plus } from '@lucide/svelte';
	import type { Direction, Insert } from './layout.ts';

	let {
		sourceX,
		sourceY,
		targetX,
		targetY,
		sourcePosition,
		targetPosition,
		markerEnd,
		data
	}: EdgeProps & {
		data?: {
			label?: string;
			insert?: Insert;
			active?: boolean;
			direction?: Direction;
			onpick?: (insert: Insert) => void;
		};
	} = $props();

	let path = $derived(
		getSmoothStepPath({
			sourceX,
			sourceY,
			targetX,
			targetY,
			sourcePosition,
			targetPosition,
			borderRadius: 12
		})
	);
	// Top to bottom: lane labels near the top of their edge, the "+" just
	// above where it arrives. Left to right: both on the last horizontal
	// stretch, the label above the line.
	let across = $derived(data?.direction === 'right');
	let label = $derived(
		across ? { x: targetX - 64, y: targetY - 22 } : { x: targetX, y: sourceY + 22 }
	);
	let plus = $derived(across ? { x: targetX - 22, y: targetY } : { x: targetX, y: targetY - 22 });
</script>

<BaseEdge path={path[0]} {markerEnd} class="!stroke-border" style="stroke-width: 1.5" />
{#if data?.label}
	<EdgeLabel x={label.x} y={label.y} class="nodrag nopan">
		<span
			class="rounded-full border border-border bg-surface px-2 py-0.5 text-[11px] font-medium text-fg-muted"
		>
			{data.label}
		</span>
	</EdgeLabel>
{/if}
{#if data?.insert && data.onpick}
	{@const insert = data.insert}
	{@const onpick = data.onpick}
	<EdgeLabel x={plus.x} y={plus.y} class="nodrag nopan">
		<button
			type="button"
			class="flex size-6 items-center justify-center rounded-full border shadow-sm hover:border-accent hover:bg-accent hover:text-white {data.active
				? 'border-accent bg-accent text-white'
				: 'border-border bg-surface text-fg-muted'}"
			aria-label={insert.label}
			aria-pressed={data.active ?? false}
			title={insert.label}
			onclick={() => onpick(insert)}
		>
			<Plus size={14} />
		</button>
	</EdgeLabel>
{/if}
