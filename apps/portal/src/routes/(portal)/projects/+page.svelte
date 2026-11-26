<script lang="ts">
	import { Badge, Card, EmptyState } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const STATUS: Record<string, string> = {
		planned: 'Planned',
		active: 'In progress',
		on_hold: 'On hold',
		completed: 'Completed'
	};
	const date = (d: string) => new Date(`${d}T00:00:00`).toLocaleDateString();
</script>

<svelte:head><title>Projects</title></svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-4">
	<h1 class="text-2xl font-semibold text-fg">Projects</h1>
	{#if data.projects.length === 0}
		<EmptyState
			title="No projects"
			description="Projects your IT provider runs for you show here."
		/>
	{/if}
	{#each data.projects as p (p.id)}
		<Card title={p.name}>
			<div class="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
				<Badge>{STATUS[p.status] ?? p.status}</Badge>
				<span>From {date(p.startOn)}{p.endOn ? ` to ${date(p.endOn)}` : ''}</span>
				<span>· {p.progress.done} of {p.progress.total} tasks done</span>
			</div>
			{#if p.description}<p class="mt-2 text-sm text-fg">{p.description}</p>{/if}
			{#if p.milestones.length > 0}
				<h2 class="mt-3 text-sm font-semibold text-fg">Milestones</h2>
				<ul class="mt-1 flex flex-col gap-1 text-sm" aria-label="Milestones of {p.name}">
					{#each p.milestones as m (m.id)}
						<li class="flex items-center justify-between gap-2">
							<span class="text-fg">{m.name}{m.dueOn ? ` · due ${date(m.dueOn)}` : ''}</span>
							{#if m.reachedAt}
								<Badge tone="success">Reached {new Date(m.reachedAt).toLocaleDateString()}</Badge>
							{:else}
								<Badge>Upcoming</Badge>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if p.tasks.length > 0}
				<h2 class="mt-3 text-sm font-semibold text-fg">For you to know</h2>
				<ul class="mt-1 flex flex-col gap-1 text-sm">
					{#each p.tasks as t (t.title)}
						<li class="flex items-center justify-between gap-2">
							<span class="text-fg {t.done ? 'line-through' : ''}">{t.title}</span>
							<span class="text-fg-muted">{t.statusName}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
	{/each}
</div>
