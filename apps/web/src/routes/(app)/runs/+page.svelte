<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card } from '@usehuxley/ui';
	import RunStatusBadge from '#lib/workflows/RunStatusBadge.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const filters = [
		{ value: '', label: 'All' },
		{ value: 'running', label: 'Running' },
		{ value: 'failed', label: 'Failed' },
		{ value: 'succeeded', label: 'Succeeded' },
		{ value: 'cancelled', label: 'Cancelled' }
	];
	let last = $derived(data.runs.at(-1));

	function when(value: string): string {
		return new Date(value).toLocaleString();
	}
</script>

<svelte:head>
	<title>Runs · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Runs</h1>
		<p class="mt-1 text-sm text-fg-muted">Every workflow run you can see, newest first.</p>
	</header>

	<form method="GET" class="flex flex-wrap gap-2" aria-label="Filter runs">
		{#each filters as filter (filter.value)}
			<button
				type="submit"
				name="status"
				value={filter.value}
				aria-pressed={data.status === filter.value}
				class="rounded-control border border-border px-3 py-1 text-sm {data.status === filter.value
					? 'bg-accent-soft text-accent'
					: 'text-fg-muted hover:text-fg'}"
			>
				{filter.label}
			</button>
		{/each}
	</form>

	<Card>
		{#if data.runs.length === 0}
			<p class="text-sm text-fg-muted">No runs.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Runs">
				{#each data.runs as run (run.id)}
					<li class="flex items-center justify-between gap-4 py-2 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/runs/[id]', { id: run.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{(run.workflowId && data.workflowNames[run.workflowId]) || 'Workflow'}
							</a>
							<p class="text-xs text-fg-muted">
								{when(run.createdAt)} · {run.workflowVersion ? `v${run.workflowVersion}` : 'test'} · {run.triggerType}
							</p>
						</div>
						<RunStatusBadge status={run.status} />
					</li>
				{/each}
			</ul>
			{#if data.runs.length === 50 && last}
				<form method="GET" class="mt-4">
					{#if data.status}<input type="hidden" name="status" value={data.status} />{/if}
					<input type="hidden" name="before" value={last.id} />
					<button type="submit" class="text-sm text-accent hover:underline">Older runs</button>
				</form>
			{/if}
		{/if}
	</Card>
</div>
