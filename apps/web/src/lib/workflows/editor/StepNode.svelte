<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';
	import { Badge } from '@usehuxley/ui';
	import { pathKey, stepSpec, summary, type Path, type Step } from './catalog.ts';
	import { stepIcon } from './icons.ts';
	import type { EditorContext } from './context.ts';
	import type { Direction } from './layout.ts';

	let {
		data
	}: {
		data: { step: Step; path: Path; editor: EditorContext; direction?: Direction };
	} = $props();

	let spec = $derived(stepSpec(data.step.type));
	let key = $derived(pathKey(data.path));
	let problems = $derived(data.editor.problems(key));
	let selected = $derived(data.editor.selected !== null && pathKey(data.editor.selected) === key);
	let Icon = $derived(stepIcon(spec?.group, data.step.type));
	let title = $derived(
		typeof data.step.name === 'string' && data.step.name ? data.step.name : String(data.step.id)
	);
</script>

<Handle
	type="target"
	position={data.direction === 'right' ? Position.Left : Position.Top}
	isConnectable={false}
	class="!opacity-0"
/>
<button
	type="button"
	class="nodrag flex h-[76px] w-[248px] items-center gap-3 rounded-card border bg-surface px-3 text-left shadow-sm transition-colors hover:border-accent {selected
		? 'border-accent ring-2 ring-accent/40'
		: problems.length > 0
			? 'border-warning'
			: 'border-border'}"
	aria-pressed={selected}
	aria-label={`Step ${String(data.step.id)}, ${spec?.label ?? String(data.step.type)}`}
	onclick={() => data.editor.select(selected ? null : data.path)}
>
	<span
		class="flex size-9 shrink-0 items-center justify-center rounded-control {spec?.group ===
		'Control flow'
			? 'bg-warning/15 text-warning'
			: spec?.group === 'Finish'
				? 'bg-danger/10 text-danger'
				: 'bg-accent-soft text-accent'}"
	>
		<Icon size={18} />
	</span>
	<span class="min-w-0 flex-1">
		<span class="block truncate text-[11px] font-medium tracking-wide text-fg-muted uppercase">
			{spec?.label ?? String(data.step.type)}
		</span>
		<span class="block truncate text-sm font-medium text-fg">{title}</span>
		{#if summary(data.step)}
			<span class="block truncate font-mono text-[11px] text-fg-muted">{summary(data.step)}</span>
		{/if}
	</span>
	{#if problems.length > 0}
		<Badge tone="warning">{problems.length}</Badge>
	{/if}
</button>
<Handle
	type="source"
	position={data.direction === 'right' ? Position.Right : Position.Bottom}
	isConnectable={false}
	class="!opacity-0"
/>
