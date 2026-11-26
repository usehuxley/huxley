<script lang="ts">
	import { enhance } from '$app/forms';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { Copy, Pencil, RefreshCw } from '@lucide/svelte';
	import { Button, Select } from '@usehuxley/ui';
	import { Widget } from '@usehuxley/widgets';
	import { RANGES } from '#lib/dashboards/spec.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	function pick(name: 'range' | 'org', value: string) {
		const url = new URL(page.url.href);
		if (value) url.searchParams.set(name, value);
		else url.searchParams.delete(name);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- the same page, new filters
		void goto(url);
	}
</script>

<svelte:head><title>{data.dashboard.name} · Huxley</title></svelte:head>

<div class="flex flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(app)/dashboards')} class="text-sm text-accent hover:underline">
				Dashboards
			</a>
			<h1 class="mt-1 text-2xl font-semibold text-fg">{data.dashboard.name}</h1>
			{#if data.dashboard.description}<p class="text-fg-muted">{data.dashboard.description}</p>{/if}
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<div class="w-40">
				<Select
					aria-label="Range"
					value={data.range}
					options={RANGES}
					onchange={(e: Event) => pick('range', (e.currentTarget as HTMLSelectElement).value)}
				/>
			</div>
			<div class="w-56">
				<Select
					aria-label="Show numbers for"
					value={data.orgId ?? ''}
					options={[{ value: '', label: 'Everyone you can see' }, ...data.orgs]}
					onchange={(e: Event) => pick('org', (e.currentTarget as HTMLSelectElement).value)}
				/>
			</div>
			<Button variant="ghost" onclick={() => invalidateAll()} aria-label="Refresh">
				<RefreshCw size={16} />
			</Button>
			{#if data.canEdit}
				<Button href={resolve('/(app)/dashboards/[id]/edit', { id: data.dashboard.id })}>
					<Pencil size={16} /> Edit
				</Button>
			{:else}
				<form method="POST" action="?/copy" use:enhance>
					<Button type="submit"><Copy size={16} /> Copy to change it</Button>
				</form>
			{/if}
		</div>
	</div>
	{#if data.dashboard.widgets.length === 0}
		<p class="text-fg-muted">No widgets yet.</p>
	{/if}
	<div class="grid grid-cols-1 gap-4 md:grid-cols-12">
		{#each data.dashboard.widgets as w (w.id)}
			<Widget
				title={w.title}
				viz={w.viz}
				width={w.width}
				answer={data.results[w.id]?.answer}
				problem={data.results[w.id]?.problem}
			/>
		{/each}
	</div>
</div>
