<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { Plus, Search } from '@lucide/svelte';
	import { Badge, Button, EmptyState, Input, Select } from '@usehuxley/ui';
	import { VIEWS, ago, categoryTone, priorityOf } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let canWrite = $derived(data.user.grantedPermissions.includes('ticket:write'));
	let queueOptions = $derived([
		{ value: '', label: 'All queues' },
		...data.queues.map((q) => ({ value: q.id, label: q.name }))
	]);

	/** This page's URL with some parameters changed (and paging reset unless given). */
	function withParams(changes: Record<string, string | number | undefined>): string {
		const url = new URL(page.url.href);
		if (!('page' in changes)) url.searchParams.delete('page');
		for (const [key, value] of Object.entries(changes)) {
			if (value === undefined || value === '' || (key === 'page' && value === 1)) {
				url.searchParams.delete(key);
			} else {
				url.searchParams.set(key, String(value));
			}
		}
		return `${url.pathname}${url.search}`;
	}

	function filterQueue(event: Event) {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same page, new query
		void goto(withParams({ queue: (event.currentTarget as HTMLSelectElement).value }), {
			reset: false
		});
	}
</script>

<svelte:head>
	<title>Tickets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Tickets</h1>
			<p class="mt-1 text-fg-muted">Requests and incidents from your clients.</p>
		</div>
		{#if canWrite}
			<Button href={resolve('/(app)/tickets/new')} variant="primary">
				<Plus size={16} /> New ticket
			</Button>
		{/if}
	</header>

	<nav aria-label="Ticket views" class="flex gap-1 overflow-x-auto border-b border-border">
		{#each VIEWS as view (view.value)}
			{@const active = data.view === view.value}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with a new query -->
			<a
				href={withParams({ view: view.value === 'mine' ? undefined : view.value })}
				aria-current={active ? 'page' : undefined}
				class="-mb-px shrink-0 border-b-2 px-3 py-2 text-sm font-medium {active
					? 'border-accent text-fg'
					: 'border-transparent text-fg-muted hover:text-fg'}"
			>
				{view.label}
			</a>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{/each}
	</nav>

	<div class="flex flex-wrap items-center gap-2">
		<form method="GET" class="flex gap-2" role="search">
			<input type="hidden" name="view" value={data.view === 'mine' ? undefined : data.view} />
			{#if data.queueId}<input type="hidden" name="queue" value={data.queueId} />{/if}
			{#if data.orgId}<input type="hidden" name="org" value={data.orgId} />{/if}
			<label for="q" class="sr-only">Search tickets</label>
			<Input id="q" name="q" value={data.search} placeholder="Title or number" class="w-64" />
			<Button type="submit"><Search size={16} /> Search</Button>
		</form>
		{#if data.queues.length > 0}
			<div class="w-48">
				<label for="queue" class="sr-only">Queue</label>
				<Select id="queue" value={data.queueId} options={queueOptions} onchange={filterQueue} />
			</div>
		{/if}
		{#if data.orgId}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with a new query -->
			<a href={withParams({ org: undefined })} class="text-sm text-accent hover:underline">
				Showing one client · show all
			</a>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{/if}
	</div>

	{#if data.tickets.length === 0}
		<EmptyState
			title={data.search ? 'No tickets match' : 'No tickets here'}
			description={data.search
				? 'Try another title or number.'
				: data.view === 'mine'
					? 'Nothing is assigned to you right now.'
					: 'Tickets in this view will show up here.'}
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Tickets">
				<thead
					class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
				>
					<tr>
						<th scope="col" class="px-4 py-2.5 font-medium">Ticket</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Client</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Status</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Priority</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Assignee</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Updated</th>
					</tr>
				</thead>
				<tbody>
					{#each data.tickets as ticket (ticket.id)}
						{@const priority = priorityOf(ticket.priority)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="max-w-md px-4 py-2.5">
								<a
									class="block truncate font-medium text-fg hover:text-accent"
									href={resolve('/(app)/tickets/[id]', { id: ticket.id })}
								>
									{ticket.title}
								</a>
								<span class="text-xs text-fg-muted tabular-nums">
									{ticket.reference} · {ticket.typeName}{ticket.queueName
										? ` · ${ticket.queueName}`
										: ''}
								</span>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{ticket.orgName}</td>
							<td class="px-4 py-2.5">
								<Badge tone={categoryTone(ticket.statusCategory)}>{ticket.statusName}</Badge>
							</td>
							<td class="px-4 py-2.5">
								<Badge tone={priority.tone}>{priority.short}</Badge>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">
								{ticket.assigneeUserName ?? ticket.assigneeTeamName ?? 'Unassigned'}
							</td>
							<td class="px-4 py-2.5 text-right whitespace-nowrap text-fg-muted">
								<time
									datetime={ticket.updatedAt}
									title={new Date(ticket.updatedAt).toLocaleString()}
								>
									{ago(ticket.updatedAt)}
								</time>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
		{#if data.pages > 1}
			<nav aria-label="Pages" class="flex items-center justify-between text-sm text-fg-muted">
				<span>{data.total} tickets · page {data.page} of {data.pages}</span>
				<span class="flex gap-2">
					{#if data.page > 1}
						<Button href={withParams({ page: data.page - 1 })} size="sm">Previous</Button>
					{/if}
					{#if data.page < data.pages}
						<Button href={withParams({ page: data.page + 1 })} size="sm">Next</Button>
					{/if}
				</span>
			</nav>
		{/if}
	{/if}
</div>
