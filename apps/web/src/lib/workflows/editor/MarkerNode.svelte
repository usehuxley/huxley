<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';
	import { Flag, Play, TriangleAlert } from '@lucide/svelte';
	import type { Direction } from './layout.ts';

	let {
		data
	}: {
		data: {
			kind: 'start' | 'end' | 'join' | 'empty';
			title?: string;
			detail?: string;
			failure?: boolean;
			direction?: Direction;
		};
	} = $props();
</script>

{#if data.kind === 'start'}
	<div
		class="flex h-[76px] w-[248px] items-center gap-3 rounded-full border border-border bg-surface-sunken px-4"
	>
		<span
			class="flex size-9 shrink-0 items-center justify-center rounded-full {data.failure
				? 'bg-danger/10 text-danger'
				: 'bg-accent text-white'}"
		>
			{#if data.failure}<TriangleAlert size={18} />{:else}<Play size={16} />{/if}
		</span>
		<span class="min-w-0">
			<span class="block text-sm font-semibold text-fg">{data.title}</span>
			<span class="block truncate text-xs text-fg-muted">{data.detail}</span>
		</span>
	</div>
	<Handle
		type="source"
		position={data.direction === 'right' ? Position.Right : Position.Bottom}
		isConnectable={false}
		class="!opacity-0"
	/>
{:else}
	<Handle
		type="target"
		position={data.direction === 'right' ? Position.Left : Position.Top}
		isConnectable={false}
		class="!opacity-0"
	/>
	{#if data.kind === 'end'}
		<div
			class="flex size-7 items-center justify-center rounded-full border border-border bg-surface-sunken text-fg-muted"
			title="End"
		>
			<Flag size={12} />
		</div>
	{:else}
		<div
			class="size-7 rounded-full border-2 border-border bg-surface {data.kind === 'empty'
				? 'border-dashed'
				: ''}"
			title={data.kind === 'empty' ? 'No steps yet' : 'The branches meet'}
		></div>
		<Handle
			type="source"
			position={data.direction === 'right' ? Position.Right : Position.Bottom}
			isConnectable={false}
			class="!opacity-0"
		/>
	{/if}
{/if}
