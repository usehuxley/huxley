<script lang="ts">
	import { resolve } from '$app/paths';
	import { FileText } from '@lucide/svelte';
	import { Badge, Card, EmptyState } from '@usehuxley/ui';
	import { STATUS, money } from '#lib/quotes/format.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Quotes · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Quotes</h1>
		<p class="mt-1 text-fg-muted">What we've offered, and your answers.</p>
	</div>
	<Card>
		{#if data.quotes.length === 0}
			<EmptyState title="No quotes" description="Quotes we send you show up here.">
				{#snippet icon()}<FileText size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Quotes">
				{#each data.quotes as q (q.id)}
					{@const status = STATUS[q.status] ?? STATUS.sent}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(portal)/quotes/[id]', { id: q.id })}
								class="font-medium text-fg hover:underline"
							>
								{q.title}
							</a>
							<p class="text-xs text-fg-muted">
								Q-{q.number} · {money(
									q.totals.oneTime,
									q.currency
								)}{#if Number(q.totals.monthly) > 0}
									+ {money(q.totals.monthly, q.currency)}/month{/if}
							</p>
						</div>
						<Badge tone={q.expired ? 'neutral' : status.tone}>
							{q.expired ? 'Expired' : status.label}
						</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
