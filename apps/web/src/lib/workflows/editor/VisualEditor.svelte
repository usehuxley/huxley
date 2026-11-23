<script lang="ts">
	import StepList from './StepList.svelte';
	import StepPanel from './StepPanel.svelte';
	import { isStep, type Path } from './catalog.ts';
	import { ownerOf } from './problems.ts';
	import type { ActionChoice, EditorContext } from './context.ts';

	let {
		draft,
		diagnostics,
		readonly = false,
		dirty = $bindable(false),
		document = $bindable(),
		actions = []
	}: {
		draft: Record<string, unknown>;
		actions?: ActionChoice[];
		diagnostics: { path: string; message: string }[];
		readonly?: boolean;
		dirty?: boolean;
		document?: Record<string, unknown>;
	} = $props();

	// The working copy; replaced whenever the saved draft changes.
	let doc = $state<Record<string, unknown>>({});
	let selected = $state<Path | null>(null);
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

	function spec(): Record<string, unknown> {
		if (!isStep(doc.spec)) doc.spec = {};
		return doc.spec as Record<string, unknown>;
	}
	function list(field: string): { steps: unknown[]; attach: () => unknown[] } {
		const existing = isStep(doc.spec) ? doc.spec[field] : undefined;
		return {
			steps: Array.isArray(existing) ? existing : [],
			attach: () => {
				const s = spec();
				if (!Array.isArray(s[field])) s[field] = [];
				return s[field] as unknown[];
			}
		};
	}

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
		},
		changed() {
			dirty = true;
		},
		problems(key) {
			return byStep[key] ?? [];
		},
		get actions() {
			return actions;
		}
	};

	let steps = $derived(list('steps'));
	let onError = $derived(list('on_error'));
</script>

<div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
	<div class="flex min-w-0 flex-col gap-4">
		{#if general.length > 0}
			<ul class="rounded-control border border-warning/40 bg-warning/5 p-2 text-sm text-fg">
				{#each general as d, i (i)}<li><code>{d.path || 'document'}</code>: {d.message}</li>{/each}
			</ul>
		{/if}
		<section>
			<h3 class="text-sm font-semibold text-fg">Steps</h3>
			<StepList
				steps={steps.steps}
				attach={steps.attach}
				path={['spec', 'steps']}
				{editor}
				label="Steps"
			/>
		</section>
		<section>
			<h3 class="text-sm font-semibold text-fg">If the run fails</h3>
			<p class="text-xs text-fg-muted">Runs when nothing else caught the error.</p>
			<StepList
				steps={onError.steps}
				attach={onError.attach}
				path={['spec', 'on_error']}
				{editor}
				label="If the run fails"
			/>
		</section>
	</div>
	<aside class="rounded-control border border-border p-3 lg:sticky lg:top-4 lg:self-start">
		<StepPanel {editor} />
	</aside>
</div>
