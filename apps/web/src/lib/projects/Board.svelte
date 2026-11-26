<script lang="ts">
	import { resolve } from '$app/paths';
	import type { components } from '@usehuxley/api-client';
	import { Badge } from '@usehuxley/ui';

	type Task = components['schemas']['ProjectTask'];
	let { tasks }: { tasks: Task[] } = $props();

	// Columns by what the status means, so custom statuses fall in place.
	const COLUMNS = [
		{ key: 'todo', label: 'To do', categories: ['new'] },
		{ key: 'doing', label: 'In progress', categories: ['open'] },
		{ key: 'blocked', label: 'Blocked', categories: ['pending'] },
		{ key: 'done', label: 'Done', categories: ['resolved', 'closed'] }
	];
</script>

<div class="grid gap-3 md:grid-cols-4">
	{#each COLUMNS as col (col.key)}
		{@const items = tasks.filter((t) => col.categories.includes(t.statusCategory))}
		<section
			class="flex flex-col gap-2 rounded-card bg-surface-sunken p-2"
			aria-label="{col.label} column"
		>
			<h3 class="flex items-center justify-between px-1 text-sm font-semibold text-fg">
				{col.label}<span class="font-normal text-fg-muted">{items.length}</span>
			</h3>
			{#each items as t (t.ticketId)}
				<a
					href={resolve('/(app)/tickets/[id]', { id: t.ticketId })}
					class="flex flex-col gap-1 rounded-control border border-border bg-surface p-2 text-sm hover:border-accent"
				>
					<span class="font-medium text-fg">{t.title}</span>
					<span class="flex flex-wrap items-center gap-1 text-xs text-fg-muted">
						{t.reference} · {t.assigneeName ?? 'Unassigned'}
						{#if t.dueOn}· due {t.dueOn}{/if}
						{#if t.waiting}<Badge tone="warning">Waiting</Badge>{/if}
					</span>
				</a>
			{:else}
				<p class="px-1 text-xs text-fg-muted">Nothing here.</p>
			{/each}
		</section>
	{/each}
</div>
