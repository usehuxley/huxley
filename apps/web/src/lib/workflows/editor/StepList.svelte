<script lang="ts">
	import AddStep from './AddStep.svelte';
	import StepCard from './StepCard.svelte';
	import { isStep, newStep, type Path } from './catalog.ts';
	import type { EditorContext } from './context.ts';

	let {
		steps,
		attach,
		path,
		editor,
		label
	}: {
		steps: unknown[];
		/** The real list in the document, created on first use. */
		attach: () => unknown[];
		path: Path;
		editor: EditorContext;
		label: string;
	} = $props();

	function add(index: number, type: string) {
		const step = newStep(type, editor.document);
		attach().splice(index, 0, step);
		editor.select([...path, index]);
		editor.changed();
	}
</script>

<ol class="flex flex-col" aria-label={label}>
	{#if !editor.readonly}
		<li>
			<AddStep
				onadd={(type: string) => add(0, type)}
				label={`Add a step at the start of ${label}`}
			/>
		</li>
	{/if}
	{#each steps as step, i (i)}
		{#if isStep(step)}
			<li>
				<StepCard {step} path={[...path, i]} {editor} />
				{#if !editor.readonly}
					<AddStep
						onadd={(type: string) => add(i + 1, type)}
						label={`Add a step after ${String(step.id)}`}
					/>
				{:else}
					<div class="h-2"></div>
				{/if}
			</li>
		{/if}
	{/each}
</ol>
