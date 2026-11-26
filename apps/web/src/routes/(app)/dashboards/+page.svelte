<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ChartColumn, Plus } from '@lucide/svelte';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let prebuilt = $derived(data.dashboards.filter((d) => d.builtin));
	let shared = $derived(data.dashboards.filter((d) => !d.builtin && !d.ownerUserId));
	let mine = $derived(data.dashboards.filter((d) => !d.builtin && d.ownerUserId));
</script>

<svelte:head><title>Dashboards · Huxley</title></svelte:head>

<div class="flex flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Dashboards</h1>
			<p class="mt-1 text-fg-muted">Numbers across tickets, time, assets and workflows.</p>
		</div>
		<form method="POST" action="?/create" use:enhance>
			<Button type="submit" variant="primary"><Plus size={16} /> New dashboard</Button>
		</form>
	</div>
	{#if data.waiting.length > 0}
		<Card title="Waiting for your approval">
			<ul class="flex flex-col gap-2" aria-label="Waiting for your approval">
				{#each data.waiting as s (s.id)}
					<li class="flex flex-wrap items-center justify-between gap-2">
						<a
							href={resolve('/(app)/dashboards/submissions/[id]', { id: s.id })}
							class="font-medium text-accent hover:underline"
						>
							{s.snapshot.name}
						</a>
						<span class="text-sm text-fg-muted">
							{s.submittedByName ?? 'Someone'} · {new Date(s.submittedAt).toLocaleDateString()}
							{#if s.portal}· for the client portal too{/if}
						</span>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}
	{#each [{ title: 'Prebuilt', list: prebuilt }, { title: 'Shared', list: shared }, { title: 'Yours', list: mine }] as group (group.title)}
		{#if group.list.length > 0}
			<Card title={group.title}>
				<ul class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3" aria-label="{group.title} dashboards">
					{#each group.list as d (d.id)}
						<li>
							<a
								href={resolve('/(app)/dashboards/[id]', { id: d.id })}
								class="flex h-full flex-col gap-1 rounded-card border border-border p-4 hover:border-accent"
							>
								<span class="flex items-center gap-2 font-medium text-fg">
									<ChartColumn size={16} class="text-fg-muted" />
									{d.name}
									{#if group.title === 'Yours'}<Badge>Personal</Badge>{/if}
								</span>
								{#if d.description}<span class="text-sm text-fg-muted">{d.description}</span>{/if}
								<span class="text-xs text-fg-subtle">{d.widgets.length} widgets</span>
							</a>
						</li>
					{/each}
				</ul>
			</Card>
		{/if}
	{/each}
</div>
