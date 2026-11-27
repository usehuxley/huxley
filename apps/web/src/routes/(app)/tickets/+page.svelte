<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { enhance } from '$app/forms';
	import { Bookmark, Plus, Search, Sparkles, Users } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { VIEWS, ago, categoryTone, priorityOf } from '#lib/tickets/labels.ts';
	import { missed } from '#lib/tickets/sla.ts';
	import SlaBar from '#lib/tickets/SlaBar.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	// SLA bars count down while the list is open.
	let now = $state(Date.now());
	$effect(() => {
		const timer = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(timer);
	});

	let canWrite = $derived(data.user.grantedPermissions.includes('ticket:write'));
	let queueOptions = $derived([
		{ value: '', label: 'All queues' },
		...data.queues.map((q) => ({ value: q.id, label: q.name }))
	]);

	let canShare = $derived(data.user.grantedPermissions.includes('servicedesk:configure'));
	let saving = $state(false);
	let selected: string[] = $state([]);
	let allSelected = $derived(
		data.tickets.length > 0 && data.tickets.every((t) => selected.includes(t.id))
	);
	// What was picked is gone once the list changes.
	$effect(() => {
		void data.tickets;
		selected = [];
	});
	let bulkAssignees = $derived([
		{ value: '', label: 'Assignee: keep' },
		{ value: `user:${data.user.id}`, label: 'Me' },
		{ value: 'none', label: 'Nobody' },
		...data.people
			.filter((p) => p.value !== data.user.id)
			.map((p) => ({ value: `user:${p.value}`, label: p.label })),
		...data.teams.map((t) => ({ value: `team:${t.value}`, label: `Team: ${t.label}` }))
	]);
	const bulkPriorities = [
		{ value: '', label: 'Priority: keep' },
		{ value: 'auto', label: 'From the matrix' },
		{ value: 'p1', label: 'P1' },
		{ value: 'p2', label: 'P2' },
		{ value: 'p3', label: 'P3' },
		{ value: 'p4', label: 'P4' }
	];
	let bulkQueues = $derived([
		{ value: '', label: 'Queue: keep' },
		{ value: 'none', label: 'No queue' },
		...data.queues.map((q) => ({ value: q.id, label: q.name }))
	]);
	let bulkStatuses = $derived([{ value: '', label: 'Status: keep' }, ...data.statuses]);
	let active = $derived(data.views.find((v) => v.id === data.saved));
	const PRIORITY_FILTERS = [
		{ value: '', label: 'All priorities' },
		{ value: 'p1', label: 'P1' },
		{ value: 'p1,p2', label: 'P1 and P2' },
		{ value: 'p2', label: 'P2' },
		{ value: 'p3', label: 'P3' },
		{ value: 'p4', label: 'P4' },
		{ value: 'p3,p4', label: 'P3 and P4' }
	];
	let priorityOptions = $derived(
		PRIORITY_FILTERS.some((p) => p.value === data.priority)
			? PRIORITY_FILTERS
			: [...PRIORITY_FILTERS, { value: data.priority, label: data.priority.toUpperCase() }]
	);

	/** A saved view's link: its filters on this page. */
	function savedHref(view: (typeof data.views)[number]): string {
		const f = view.filters as Record<string, string | undefined>;
		const query = Object.entries({
			view: f.view === 'mine' ? undefined : f.view,
			queue: f.queueId,
			org: f.orgId,
			q: f.search,
			priority: f.priority,
			saved: view.id
		})
			.filter(([, value]) => value)
			.map(([key, value]) => `${key}=${encodeURIComponent(value!)}`)
			.join('&');
		return `${page.url.pathname}?${query}`;
	}

	/** This page's URL with some parameters changed (and paging reset unless given). */
	function withParams(changes: Record<string, string | number | undefined>): string {
		const url = new URL(page.url.href);
		if (!('page' in changes)) url.searchParams.delete('page');
		// Changing a filter leaves the saved view.
		if (!('saved' in changes)) url.searchParams.delete('saved');
		for (const [key, value] of Object.entries(changes)) {
			if (value === undefined || value === '' || (key === 'page' && value === 1)) {
				url.searchParams.delete(key);
			} else {
				url.searchParams.set(key, String(value));
			}
		}
		return `${url.pathname}${url.search}`;
	}

	function filterPriority(event: Event) {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same page, new query
		void goto(withParams({ priority: (event.currentTarget as HTMLSelectElement).value }), {
			reset: false
		});
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
			{@const active = !data.saved && data.view === view.value}
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
		{#if data.views.length > 0}
			<span class="my-2 w-px shrink-0 bg-border" aria-hidden="true"></span>
		{/if}
		{#each data.views as view (view.id)}
			{@const on = data.saved === view.id}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with a new query -->
			<a
				href={savedHref(view)}
				aria-current={on ? 'page' : undefined}
				title={view.shared ? 'Shared with everyone' : 'Yours'}
				class="-mb-px flex shrink-0 items-center gap-1.5 border-b-2 px-3 py-2 text-sm font-medium {on
					? 'border-accent text-fg'
					: 'border-transparent text-fg-muted hover:text-fg'}"
			>
				{#if view.shared}<Users size={14} aria-hidden="true" />{/if}
				{view.name}
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
			<Button
				href={`${resolve('/(app)/tickets/meaning')}?${new URLSearchParams({
					...(data.search ? { q: data.search } : {}),
					...(data.orgId ? { org: data.orgId } : {})
				})}`}
				variant="ghost"
				title="Find tickets about the same thing, in other words"
			>
				<Sparkles size={16} /> By meaning
			</Button>
		</form>
		<div class="w-40">
			<label for="priority" class="sr-only">Priority</label>
			<Select
				id="priority"
				value={data.priority}
				options={priorityOptions}
				onchange={filterPriority}
			/>
		</div>
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
		<div class="ml-auto flex gap-2">
			{#if active && (!active.shared || canShare)}
				<form
					method="POST"
					action="?/deleteView"
					use:enhance={() =>
						async ({ result, update }) => {
							if (result.type === 'failure') {
								const failed = result.data as { message?: string } | undefined;
								toast.push({ title: failed?.message ?? 'Could not delete it.', tone: 'danger' });
							}
							await update();
						}}
				>
					<input type="hidden" name="id" value={active.id} />
					<Button type="submit" variant="ghost">Delete view</Button>
				</form>
			{:else if !active}
				<Button onclick={() => (saving = true)}><Bookmark size={16} /> Save view</Button>
			{/if}
		</div>
	</div>

	{#if canWrite && selected.length > 0}
		<form
			id="bulk"
			method="POST"
			action="?/bulk"
			aria-label="Change selected tickets"
			class="flex flex-wrap items-center gap-2 rounded-card border border-accent/40 bg-accent-soft p-3"
			use:enhance={() =>
				async ({ result, update }) => {
					if (result.type === 'success') {
						const done = (
							result.data as { bulk?: { changed: number; failed: number; reason: string | null } }
						).bulk;
						if (done && done.failed > 0) {
							toast.push({
								title: `Changed ${done.changed}; ${done.failed} couldn’t be`,
								description: done.reason ?? undefined,
								tone: 'warning'
							});
						} else if (done) {
							toast.push({ title: `Changed ${done.changed} tickets.`, tone: 'success' });
						}
					} else if (result.type === 'failure') {
						const failed = result.data as { message?: string } | undefined;
						toast.push({ title: failed?.message ?? 'Could not change them.', tone: 'danger' });
					}
					await update({ reset: false });
				}}
		>
			<span class="text-sm font-medium text-fg">{selected.length} selected</span>
			<Select name="assignee" aria-label="Assignee" value="" options={bulkAssignees} />
			<Select name="priority" aria-label="Set priority" value="" options={bulkPriorities} />
			{#if data.queues.length > 0}
				<Select name="queue" aria-label="Set queue" value="" options={bulkQueues} />
			{/if}
			{#if data.statuses.length > 0}
				<Select name="status" aria-label="Set status" value="" options={bulkStatuses} />
			{/if}
			<Button type="submit" variant="primary">Apply</Button>
			<Button variant="ghost" onclick={() => (selected = [])}>Clear</Button>
		</form>
	{:else if canWrite}
		<form id="bulk" method="POST" action="?/bulk" hidden></form>
	{/if}

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
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						{#if canWrite}
							<th scope="col" class="w-10 py-2.5 pl-4">
								<input
									type="checkbox"
									aria-label="Select all"
									checked={allSelected}
									onchange={(e) =>
										(selected = e.currentTarget.checked ? data.tickets.map((t) => t.id) : [])}
								/>
							</th>
						{/if}
						<th scope="col" class="px-4 py-2.5 font-medium">Ticket</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Client</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Status</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Priority</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Assignee</th>
						<th scope="col" class="px-4 py-2.5 font-medium">SLA</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Updated</th>
					</tr>
				</thead>
				<tbody>
					{#each data.tickets as ticket (ticket.id)}
						{@const priority = priorityOf(ticket.priority)}
						<tr
							class="border-b border-border last:border-0 {selected.includes(ticket.id)
								? 'bg-accent-soft'
								: 'hover:bg-surface-sunken'}"
						>
							{#if canWrite}
								<td class="py-2.5 pl-4">
									<input
										type="checkbox"
										form="bulk"
										name="ids"
										value={ticket.id}
										aria-label={`Select ${ticket.reference}`}
										bind:group={selected}
									/>
								</td>
							{/if}
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
								<span class="flex flex-wrap gap-1">
									<Badge tone={categoryTone(ticket.statusCategory)}>{ticket.statusName}</Badge>
									{#if missed(ticket)}<Badge tone="danger">SLA missed</Badge>{/if}
								</span>
							</td>
							<td class="px-4 py-2.5">
								<Badge tone={priority.tone}>{priority.short}</Badge>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">
								{ticket.assigneeUserName ?? ticket.assigneeTeamName ?? 'Unassigned'}
							</td>
							<td class="px-4 py-2.5">
								<SlaBar
									createdAt={ticket.createdAt}
									dueAt={ticket.resolutionMet == null ? ticket.resolutionDueAt : null}
									paused={ticket.slaPaused}
									{now}
								/>
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
				<span>
					{data.total.toLocaleString()}{data.totalCapped ? '+' : ''} tickets · page {data.page} of {data.pages}
				</span>
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

<Dialog bind:open={saving} title="Save this view">
	<form
		method="POST"
		action="?/saveView"
		class="flex flex-col gap-4"
		use:enhance={() =>
			async ({ result, update }) => {
				if (result.type === 'failure') {
					const failed = result.data as { message?: string } | undefined;
					toast.push({ title: failed?.message ?? 'Could not save it.', tone: 'danger' });
				} else {
					saving = false;
				}
				await update({ reset: false });
			}}
	>
		<input type="hidden" name="view" value={data.view} />
		<input type="hidden" name="queueId" value={data.queueId} />
		<input type="hidden" name="orgId" value={data.orgId} />
		<input type="hidden" name="search" value={data.search} />
		<input type="hidden" name="priority" value={data.priority} />
		<p class="text-sm text-fg-muted">
			Saves the filters you have now: {VIEWS.find((v) => v.value === data.view)
				?.label}{data.priority
				? `, ${data.priority.toUpperCase().replace(',', ' and ')}`
				: ''}{data.queueId
				? `, ${data.queues.find((q) => q.id === data.queueId)?.name ?? 'a queue'}`
				: ''}{data.orgId ? ', one client' : ''}{data.search ? `, “${data.search}”` : ''}.
		</p>
		<Field
			label="Name"
			for="view-name"
			error={(form as { errors?: Record<string, string> } | null)?.errors?.name}
		>
			<Input
				id="view-name"
				name="name"
				required
				maxlength={60}
				placeholder="e.g. Urgent, unassigned"
			/>
		</Field>
		{#if canShare}
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="shared" />
				Share with everyone
			</label>
		{/if}
		<div class="flex justify-end gap-2">
			<Button onclick={() => (saving = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
