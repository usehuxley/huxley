<script lang="ts">
	import { Badge } from '@usehuxley/ui';
	import StepList from './StepList.svelte';
	import { isStep, pathKey, stepSpec, summary, type Path, type Step } from './catalog.ts';
	import type { EditorContext } from './context.ts';

	let { step, path, editor }: { step: Step; path: Path; editor: EditorContext } = $props();

	let spec = $derived(stepSpec(step.type));
	let key = $derived(pathKey(path));
	let problems = $derived(editor.problems(key));
	let selected = $derived(editor.selected !== null && pathKey(editor.selected) === key);

	type Lane = { path: Path; label: string; steps: unknown[]; attach: () => unknown[] };

	/** The list at `owner[field]`, created when first needed. */
	function listOf(owner: Step, field: string): { steps: unknown[]; attach: () => unknown[] } {
		const existing = owner[field];
		return {
			steps: Array.isArray(existing) ? existing : [],
			attach: () => {
				if (!Array.isArray(owner[field])) owner[field] = [];
				return owner[field] as unknown[];
			}
		};
	}

	function lanes(): Lane[] {
		const result: Lane[] = [];
		const add = (owner: Step, field: string, lanePath: Path, label: string) =>
			result.push({ path: lanePath, label, ...listOf(owner, field) });
		if (spec?.lanes === 'cases' && Array.isArray(step.cases)) {
			step.cases.forEach((c, i) => {
				if (!isStep(c)) return;
				const when = typeof c.when === 'string' ? c.when : JSON.stringify(c.when);
				add(c, 'steps', [...path, 'cases', i, 'steps'], `When ${when}`);
			});
		}
		if (spec?.lanes === 'branches' && Array.isArray(step.branches)) {
			step.branches.forEach((b, i) => {
				if (isStep(b)) add(b, 'steps', [...path, 'branches', i, 'steps'], `Branch ${String(b.id)}`);
			});
		}
		for (const block of spec?.blocks ?? []) {
			// Optional blocks (else, finally) show once they have steps, or while selected.
			if (!Array.isArray(step[block.path]) && !selected) continue;
			add(step, block.path, [...path, block.path], block.label);
		}
		return result;
	}
</script>

<div
	class="rounded-control border bg-surface {selected
		? 'border-accent ring-1 ring-accent'
		: problems.length > 0
			? 'border-warning'
			: 'border-border'}"
>
	<button
		type="button"
		class="flex w-full items-start justify-between gap-3 px-3 py-2 text-left"
		aria-pressed={selected}
		aria-label={`Step ${String(step.id)}, ${spec?.label ?? String(step.type)}`}
		onclick={() => editor.select(selected ? null : path)}
	>
		<span class="min-w-0">
			<span class="block text-xs font-medium tracking-wide text-fg-muted uppercase">
				{spec?.label ?? String(step.type)}
			</span>
			<span class="block text-sm font-medium text-fg">
				{typeof step.name === 'string' && step.name ? step.name : String(step.id)}
			</span>
			{#if summary(step)}
				<span class="block truncate font-mono text-xs text-fg-muted">{summary(step)}</span>
			{/if}
		</span>
		{#if problems.length > 0}
			<Badge tone="warning">{problems.length} problem{problems.length === 1 ? '' : 's'}</Badge>
		{/if}
	</button>
	{#if spec?.blocks || spec?.lanes}
		<div class="flex gap-3 overflow-x-auto border-t border-border p-3">
			{#each lanes() as lane (pathKey(lane.path))}
				<div class="min-w-56 flex-1 rounded-control bg-surface-sunken p-2">
					<p class="px-1 text-xs font-semibold text-fg-muted">{lane.label}</p>
					<StepList
						steps={lane.steps}
						attach={lane.attach}
						path={lane.path}
						{editor}
						label={lane.label}
					/>
				</div>
			{/each}
		</div>
	{/if}
</div>
