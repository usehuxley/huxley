<script lang="ts">
	import { resolve } from '$app/paths';
	import { ChartColumn } from '@lucide/svelte';
	import { Card, EmptyState } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Reports · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Reports</h1>
		<p class="mt-1 text-fg-muted">How things stand for {data.me.orgName}.</p>
	</div>
	<Card>
		{#if data.dashboards.length === 0}
			<EmptyState title="No reports yet" description="Your service desk hasn’t shared any.">
				{#snippet icon()}<ChartColumn size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Reports">
				{#each data.dashboards as d (d.id)}
					<li class="py-3 first:pt-0">
						<a
							href={resolve('/(portal)/dashboards/[id]', { id: d.id })}
							class="font-medium text-fg hover:underline"
						>
							{d.name}
						</a>
						{#if d.description}<p class="text-sm text-fg-muted">{d.description}</p>{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
