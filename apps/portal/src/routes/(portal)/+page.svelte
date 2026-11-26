<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus, Ticket } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState } from '@usehuxley/ui';
	import { statusTone, when } from '#lib/tickets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let firstName = $derived(data.me.name.split(' ')[0] || data.me.name);
</script>

<svelte:head>
	<title>Home · Support portal</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-fg">How can we help, {firstName}?</h1>
			<p class="mt-1 text-fg-muted">Support for {data.me.orgName}, in one place.</p>
		</div>
		<Button href={resolve('/(portal)/tickets/new')} variant="primary">
			<Plus size={16} /> Raise a ticket
		</Button>
	</div>

	<Card title="Your open tickets">
		{#if data.open.length === 0}
			<EmptyState title="Nothing open" description="Raise a ticket when you need help.">
				{#snippet icon()}<Ticket size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Open tickets">
				{#each data.open as t (t.id)}
					<li class="flex items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(portal)/tickets/[id]', { id: t.id })}
								class="font-medium text-fg hover:underline"
							>
								{t.title}
							</a>
							<p class="text-xs text-fg-muted">{t.reference} · updated {when(t.updatedAt)}</p>
						</div>
						<Badge tone={statusTone(t.statusCategory)}>{t.statusName}</Badge>
					</li>
				{/each}
			</ul>
			<p class="mt-3 text-sm">
				<a href={resolve('/(portal)/tickets')} class="text-accent hover:underline">
					All your tickets
				</a>
			</p>
		{/if}
	</Card>
</div>
