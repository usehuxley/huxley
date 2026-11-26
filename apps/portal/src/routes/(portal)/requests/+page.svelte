<script lang="ts">
	import { resolve } from '$app/paths';
	import { ClipboardList } from '@lucide/svelte';
	import { Badge, Card, EmptyState, type BadgeTone } from '@usehuxley/ui';
	import { when } from '#lib/tickets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Form = (typeof data.forms)[number];
	let groups = $derived(
		Object.entries(
			data.forms.reduce<Record<string, Form[]>>((by, f) => {
				const key = f.category ?? 'Other';
				(by[key] ??= []).push(f);
				return by;
			}, {})
		)
	);

	const STATUS: Record<string, { label: string; tone: BadgeTone }> = {
		received: { label: 'Received', tone: 'info' },
		in_progress: { label: 'In progress', tone: 'info' },
		done: { label: 'Done', tone: 'success' },
		failed: { label: 'Being looked at', tone: 'warning' }
	};
</script>

<svelte:head>
	<title>Request something · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Request something</h1>
		<p class="mt-1 text-fg-muted">Ask for what you need, and we’ll take it from there.</p>
	</div>
	{#if data.sent}
		<p class="rounded-card border border-border bg-surface p-3 text-sm text-fg" role="status">
			Thanks: your request is in. You can follow it below.
		</p>
	{/if}

	{#if data.forms.length === 0}
		<Card>
			<EmptyState
				title="Nothing to request here yet"
				description="Raise a ticket and tell us what you need."
			>
				{#snippet icon()}<ClipboardList size={24} />{/snippet}
			</EmptyState>
		</Card>
	{:else}
		{#each groups as [category, forms] (category)}
			<section aria-label={category} class="flex flex-col gap-3">
				<h2 class="text-sm font-semibold tracking-wide text-fg-muted uppercase">{category}</h2>
				<div class="grid gap-3 sm:grid-cols-2">
					{#each forms as f (f.id)}
						<a
							href={resolve('/(portal)/requests/[id]', { id: f.id })}
							class="rounded-card border border-border bg-surface p-4 shadow-card hover:border-accent"
						>
							<span class="font-medium text-fg">{f.name}</span>
							{#if f.description}<p class="mt-1 text-sm text-fg-muted">{f.description}</p>{/if}
						</a>
					{/each}
				</div>
			</section>
		{/each}
	{/if}

	{#if data.requests.length > 0}
		<Card title="Your requests">
			<ul class="flex flex-col divide-y divide-border" aria-label="Your requests">
				{#each data.requests as r (r.id)}
					<li class="flex items-center justify-between gap-3 py-3 first:pt-0">
						<div>
							<span class="font-medium text-fg">{r.formName}</span>
							<p class="text-xs text-fg-muted">Asked {when(r.createdAt)}</p>
						</div>
						<Badge tone={STATUS[r.status]?.tone ?? 'neutral'}>
							{STATUS[r.status]?.label ?? r.status}
						</Badge>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}
</div>
