<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus, Ticket } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Input } from '@usehuxley/ui';
	import { statusTone, when } from '#lib/tickets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let canSeeCompany = $derived(data.me.permissions.includes('portal.ticket:read:company'));
	const link = (status: string, company: boolean) =>
		`${resolve('/(portal)/tickets')}?status=${status}${company ? '&scope=company' : ''}`;
</script>

<svelte:head>
	<title>Tickets · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-fg">
				{data.company ? `${data.me.orgName}’s tickets` : 'Your tickets'}
			</h1>
			<p class="mt-1 text-sm text-fg-muted">What you’ve asked for, and where it’s up to.</p>
		</div>
		<Button href={resolve('/(portal)/tickets/new')} variant="primary">
			<Plus size={16} /> Raise a ticket
		</Button>
	</header>

	<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
	<nav aria-label="Ticket filters" class="flex flex-wrap items-center gap-2 text-sm">
		<a
			href={link('open', data.company)}
			aria-current={data.status === 'open' ? 'page' : undefined}
			class="rounded-control px-3 py-1.5 {data.status === 'open'
				? 'bg-surface-sunken font-medium text-fg'
				: 'text-fg-muted hover:text-fg'}"
		>
			Open
		</a>
		<a
			href={link('closed', data.company)}
			aria-current={data.status === 'closed' ? 'page' : undefined}
			class="rounded-control px-3 py-1.5 {data.status === 'closed'
				? 'bg-surface-sunken font-medium text-fg'
				: 'text-fg-muted hover:text-fg'}"
		>
			Resolved and closed
		</a>
		{#if canSeeCompany}
			<span class="mx-2 h-4 w-px bg-border"></span>
			<a
				href={link(data.status, !data.company)}
				class="rounded-control px-3 py-1.5 text-accent hover:underline"
			>
				{data.company ? 'Just mine' : `All of ${data.me.orgName}’s`}
			</a>
		{/if}
	</nav>
	<!-- eslint-enable svelte/no-navigation-without-resolve -->

	<form method="GET" class="flex gap-2" role="search">
		<input type="hidden" name="status" value={data.status} />
		{#if data.company}<input type="hidden" name="scope" value="company" />{/if}
		<label for="ticket-search" class="sr-only">Search tickets</label>
		<Input
			id="ticket-search"
			name="q"
			value={data.q}
			placeholder="Title or reference"
			class="w-64"
		/>
		<Button type="submit">Search</Button>
	</form>

	<Card>
		{#if data.tickets.length === 0}
			<EmptyState
				title={data.status === 'open' ? 'Nothing open' : 'Nothing here yet'}
				description={data.status === 'open' ? 'Raise a ticket when you need help.' : ''}
			>
				{#snippet icon()}<Ticket size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Tickets">
				{#each data.tickets as t (t.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(portal)/tickets/[id]', { id: t.id })}
								class="font-medium text-fg hover:underline"
							>
								{t.title}
							</a>
							<p class="text-xs text-fg-muted">
								{t.reference} · updated {when(t.updatedAt)}{data.company && t.requesterName
									? ` · ${t.requesterName}`
									: ''}
							</p>
						</div>
						<Badge tone={statusTone(t.statusCategory)}>{t.statusName}</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
