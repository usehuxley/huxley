<script lang="ts">
	import StepPicker from './StepPicker.svelte';
	import WorkflowCanvas from './WorkflowCanvas.svelte';
	import { insertStep } from './insert.ts';
	import type { Insert } from './layout.ts';
	import StepPanel from './StepPanel.svelte';
	import type { Path } from './catalog.ts';
	import { ownerOf } from './problems.ts';
	import type { ActionChoice, EditorContext, PreviewResult } from './context.ts';

	let {
		draft,
		diagnostics,
		readonly = false,
		dirty = $bindable(false),
		document = $bindable(),
		actions = [],
		modules = [],
		preview
	}: {
		draft: Record<string, unknown>;
		actions?: ActionChoice[];
		/** Active feature modules (their steps are offered). */
		modules?: string[];
		preview?: (expression: unknown) => Promise<PreviewResult>;
		diagnostics: { path: string; message: string }[];
		readonly?: boolean;
		dirty?: boolean;
		document?: Record<string, unknown>;
	} = $props();

	// The working copy; replaced whenever the saved draft changes.
	let doc = $state<Record<string, unknown>>({});
	let selected = $state<Path | null>(null);
	/** The "+" whose step picker shows in the side panel. */
	let picking = $state<Insert | null>(null);
	$effect(() => {
		doc = structuredClone<Record<string, unknown>>($state.snapshot(draft));
		dirty = false;
	});
	$effect(() => {
		document = doc;
	});

	let byStep = $derived.by(() => {
		const map: Record<string, string[]> = {};
		for (const d of diagnostics) {
			const owner = ownerOf(d.path);
			if (owner) map[owner] = [...(map[owner] ?? []), d.message];
		}
		return map;
	});
	let general = $derived(diagnostics.filter((d) => !ownerOf(d.path)));

	const editor: EditorContext = {
		get document() {
			return doc;
		},
		get selected() {
			return selected;
		},
		get readonly() {
			return readonly;
		},
		select(path) {
			selected = path;
			picking = null;
		},
		changed() {
			dirty = true;
		},
		problems(key) {
			return byStep[key] ?? [];
		},
		get preview() {
			return preview;
		},
		get actions() {
			return actions;
		}
	};
</script>

<div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
	<div class="flex min-w-0 flex-col gap-4">
		{#if general.length > 0}
			<ul class="rounded-control border border-warning/40 bg-warning/5 p-2 text-sm text-fg">
				{#each general as d, i (i)}<li><code>{d.path || 'document'}</code>: {d.message}</li>{/each}
			</ul>
		{/if}
		<WorkflowCanvas
			{editor}
			{picking}
			onpick={(insert: Insert) => {
				selected = null;
				picking = insert;
			}}
		/>
	</div>
	<aside class="rounded-control border border-border p-3 lg:sticky lg:top-4 lg:self-start">
		{#if picking}
			{@const insert = picking}
			<StepPicker
				where={insert.label}
				{modules}
				oncancel={() => (picking = null)}
				onpick={(type: string) => {
					const path = insertStep(doc, insert, type);
					picking = null;
					selected = path;
					dirty = true;
				}}
			/>
		{:else}
			<StepPanel {editor} />
		{/if}
	</aside>
</div>
