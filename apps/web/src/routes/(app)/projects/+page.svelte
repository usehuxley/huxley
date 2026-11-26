<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState } from '@usehuxley/ui';
	import { HEALTH, STATUSES, label } from '#lib/projects/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head><title>Projects · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Projects</h1>
			<p class="mt-1 text-fg-muted">Client projects: their tasks, milestones and budget.</p>
		</div>
		<div class="flex gap-2">
			<Button href={resolve('/(app)/projects/capacity')} variant="ghost">Capacity</Button>
			{#if data.canManage}
				<Button href={resolve('/(app)/projects/new')} variant="primary">
					<Plus size={16} /> New project
				</Button>
			{/if}
		</div>
	</div>
	{#if data.projects.length === 0}
		<EmptyState title="No projects yet" description="Start one from a template, or from scratch." />
	{:else}
		<Card>
			<ul class="flex flex-col divide-y divide-border" aria-label="Projects">
				{#each data.projects as p (p.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/projects/[id]', { id: p.id })}
								class="font-medium text-accent hover:underline"
							>
								{p.name}
							</a>
							<p class="text-sm text-fg-muted">
								{p.orgName} · {label(STATUSES, p.status)} · from {p.startOn}
							</p>
						</div>
						<div class="flex flex-wrap items-center gap-2 text-sm">
							<span class="text-fg-muted">{p.progress.done}/{p.progress.total} tasks</span>
							{#if p.burnPercent !== null && p.burnPercent !== undefined}
								<span class="text-fg-muted tabular-nums">{p.burnPercent}% of budget</span>
							{/if}
							<Badge tone={HEALTH[p.health.level]?.[1] ?? 'neutral'}>
								{HEALTH[p.health.level]?.[0] ?? p.health.level}
							</Badge>
						</div>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}
</div>
