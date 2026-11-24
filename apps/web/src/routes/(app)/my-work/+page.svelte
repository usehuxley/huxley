<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card, EmptyState } from '@usehuxley/ui';
	import TicketRows from '#lib/tickets/TicketRows.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let work = $derived(data.work);
	// New tickets show as new already; these need more than starting.
	let urgent = $derived(Object.values(work.attention).filter((why) => why !== 'new').length);
</script>

<svelte:head>
	<title>My work · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">My work</h1>
		<p class="mt-1 text-fg-muted">
			{work.mine.length === 0
				? 'Nothing assigned to you.'
				: `${work.mine.length} open ticket${work.mine.length === 1 ? '' : 's'}${urgent > 0 ? `, ${urgent} needing attention` : ''}.`}
		</p>
	</div>

	<Card
		title="Assigned to me"
		description="What needs attention first: missed SLAs, replies, then what's due."
	>
		{#if work.mine.length === 0}
			<EmptyState title="All clear" description="Tickets assigned to you show up here." />
		{:else}
			<TicketRows tickets={work.mine} attention={work.attention} label="Assigned to me" />
		{/if}
	</Card>

	{#if work.team.length > 0}
		<Card title="My teams" description="Assigned to a team you're in, but to nobody in it yet.">
			<TicketRows tickets={work.team} label="My teams" showAssignee />
		</Card>
	{/if}

	{#each work.queues as queue (queue.queueId)}
		<Card
			title={queue.name}
			description={queue.unassigned === 0
				? undefined
				: `${queue.unassigned} unassigned ticket${queue.unassigned === 1 ? '' : 's'}.`}
		>
			{#snippet actions()}
				<!-- eslint-disable svelte/no-navigation-without-resolve -->
				<a
					class="text-sm text-accent hover:underline"
					href={resolve('/(app)/tickets') + `?view=unassigned&queue=${queue.queueId}`}
				>
					View all
				</a>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			{/snippet}
			{#if queue.tickets.length > 0}
				<TicketRows tickets={queue.tickets} label={`Unassigned in ${queue.name}`} />
			{:else}
				<p class="text-sm text-fg-muted">Nothing waiting.</p>
			{/if}
		</Card>
	{/each}
</div>
