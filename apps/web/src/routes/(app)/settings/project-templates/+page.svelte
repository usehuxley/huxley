<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const count = (t: (typeof data.templates)[number]) =>
		(t.structure.phases ?? []).reduce((n, p) => n + (p.tasks ?? []).length, 0);
</script>

<svelte:head><title>Project templates · Settings · Huxley</title></svelte:head>

<Card
	title="Project templates"
	description="Phases, tasks with estimates, and milestones that new projects start from."
>
	<div class="flex flex-col gap-3">
		<div>
			<Button href={resolve('/(app)/settings/project-templates/[id]', { id: 'new' })}>
				<Plus size={16} /> New template
			</Button>
		</div>
		{#if data.templates.length === 0}
			<p class="text-sm text-fg-muted">No templates yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Templates">
				{#each data.templates as t (t.id)}
					<li class="flex items-center justify-between gap-2 py-2 text-sm">
						<a
							href={resolve('/(app)/settings/project-templates/[id]', { id: t.id })}
							class="text-accent hover:underline"
						>
							{t.name}
						</a>
						<span class="text-fg-muted">
							{(t.structure.phases ?? []).length} phases · {count(t)} tasks · {(
								t.structure.milestones ?? []
							).length} milestones
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
</Card>
