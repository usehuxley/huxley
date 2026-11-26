<script lang="ts">
	import { untrack } from 'svelte';
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowDown, ArrowUp, Pencil, Plus, Trash2 } from '@lucide/svelte';
	import { Button, Card, Dialog, Field, Input, Select } from '@usehuxley/ui';
	import WidgetForm from '#lib/dashboards/WidgetForm.svelte';
	import { RANGES, type DashboardWidget } from '#lib/dashboards/spec.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	// Edited locally, then saved.
	const d = untrack(() => data.dashboard);
	let name = $state(d.name);
	let description = $state(d.description ?? '');
	let range = $state(d.range);
	let widgets = $state<DashboardWidget[]>(d.widgets.map((w) => ({ ...w })));
	let editing = $state<{ index: number | null } | null>(null);
	let dialogOpen = $state(false);
	function edit(index: number | null) {
		editing = { index };
		dialogOpen = true;
	}
	function close() {
		dialogOpen = false;
		editing = null;
	}

	let payload = $derived(
		JSON.stringify({ name, description: description || null, range, widgets, version: d.version })
	);

	function move(i: number, by: number) {
		const j = i + by;
		if (j < 0 || j >= widgets.length) return;
		[widgets[i], widgets[j]] = [widgets[j], widgets[i]];
	}
	const sourceLabel = (key: string) => data.sources.find((s) => s.key === key)?.label ?? key;
</script>

<svelte:head><title>Edit {d.name} · Huxley</title></svelte:head>

<div class="flex max-w-3xl flex-col gap-6">
	<div>
		<a
			href={resolve('/(app)/dashboards/[id]', { id: d.id })}
			class="text-sm text-accent hover:underline"
		>
			{d.name}
		</a>
		<h1 class="mt-1 text-2xl font-semibold text-fg">Edit dashboard</h1>
	</div>
	<Card>
		<div class="flex flex-col gap-4">
			<Field label="Name" for="d-name">
				<Input id="d-name" bind:value={name} maxlength={100} />
			</Field>
			<Field label="Description" for="d-desc"><Input id="d-desc" bind:value={description} /></Field>
			<Field label="Range it opens with" for="d-range">
				<Select id="d-range" bind:value={range} options={RANGES} />
			</Field>
		</div>
	</Card>
	<Card title="Widgets">
		{#snippet actions()}
			<Button size="sm" onclick={() => edit(null)}>
				<Plus size={14} /> Add a widget
			</Button>
		{/snippet}
		{#if widgets.length === 0}
			<p class="text-sm text-fg-muted">No widgets yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Widgets">
				{#each widgets as w, i (w.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2">
						<span>
							<span class="font-medium text-fg">{w.title}</span>
							<span class="text-sm text-fg-muted">· {sourceLabel(w.spec.source)} · {w.viz}</span>
						</span>
						<span class="flex gap-1">
							<Button size="sm" variant="ghost" aria-label="Move up" onclick={() => move(i, -1)}>
								<ArrowUp size={14} />
							</Button>
							<Button size="sm" variant="ghost" aria-label="Move down" onclick={() => move(i, 1)}>
								<ArrowDown size={14} />
							</Button>
							<Button size="sm" variant="ghost" aria-label="Edit {w.title}" onclick={() => edit(i)}>
								<Pencil size={14} />
							</Button>
							<Button
								size="sm"
								variant="ghost"
								aria-label="Remove {w.title}"
								onclick={() => widgets.splice(i, 1)}
							>
								<Trash2 size={14} />
							</Button>
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	<div class="flex justify-between gap-2">
		<form method="POST" action="?/delete" use:enhance>
			<Button type="submit" variant="danger">Delete dashboard</Button>
		</form>
		<form method="POST" action="?/save" use:enhance>
			<input type="hidden" name="dashboard" value={payload} />
			<Button type="submit" variant="primary">Save</Button>
		</form>
	</div>
</div>

<Dialog bind:open={dialogOpen} title={editing?.index === null ? 'Add a widget' : 'Edit widget'}>
	{#if editing}
		<WidgetForm
			sources={data.sources}
			widget={editing.index === null ? null : widgets[editing.index]}
			oncancel={close}
			onsave={(w: DashboardWidget) => {
				if (editing?.index === null || editing?.index === undefined) widgets.push(w);
				else widgets[editing.index] = w;
				close();
			}}
		/>
	{/if}
</Dialog>
