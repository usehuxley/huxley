<script lang="ts">
	import { resolve } from '$app/paths';
	import { Download, Receipt } from '@lucide/svelte';
	import { Badge, Card, EmptyState, type BadgeTone } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const STATE: Record<string, { label: string; tone: BadgeTone }> = {
		paid: { label: 'Paid', tone: 'success' },
		overdue: { label: 'Overdue', tone: 'danger' },
		due: { label: 'Due', tone: 'info' },
		credit: { label: 'Credit', tone: 'neutral' }
	};
	const money = (amount: string, currency: string) =>
		Number(amount).toLocaleString(undefined, { style: 'currency', currency });
	let owed = $derived(
		data.invoices
			.filter((i) => i.state === 'due' || i.state === 'overdue')
			.reduce((t, i) => t + Number(i.amountDue ?? i.total), 0)
	);
	let currency = $derived(data.invoices[0]?.currency ?? 'USD');
</script>

<svelte:head>
	<title>Invoices · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Invoices</h1>
		<p class="mt-1 text-fg-muted">
			{#if owed > 0}{money(String(owed), currency)} to pay.{:else}Our invoices to you, and what's
				paid.{/if}
		</p>
	</div>
	<Card>
		{#if data.invoices.length === 0}
			<EmptyState title="No invoices" description="Invoices we send you show up here.">
				{#snippet icon()}<Receipt size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Invoices">
				{#each data.invoices as i (i.id)}
					{@const state = STATE[i.state] ?? STATE.due}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<p class="font-medium text-fg">
								INV-{i.number}{i.kind === 'credit_memo' ? ' (credit memo)' : ''}
							</p>
							<p class="text-xs text-fg-muted">
								{money(i.total, i.currency)}{i.issuedOn ? ` · issued ${i.issuedOn}` : ''}{i.dueOn &&
								i.state !== 'paid' &&
								i.kind !== 'credit_memo'
									? ` · due ${i.dueOn}`
									: ''}{i.amountDue && i.state !== 'paid' && i.amountDue !== i.total
									? ` · ${money(i.amountDue, i.currency)} left to pay`
									: ''}
							</p>
						</div>
						<div class="flex items-center gap-2">
							<Badge tone={state.tone}>{state.label}</Badge>
							<a
								href="{resolve('/(portal)/invoices')}/{i.id}/pdf"
								class="inline-flex items-center gap-1 text-sm text-accent hover:underline"
								aria-label="Download INV-{i.number}"
							>
								<Download size={14} /> PDF
							</a>
							{#if i.payUrl && (i.state === 'due' || i.state === 'overdue')}
								<!-- eslint-disable svelte/no-navigation-without-resolve -- the accounting system's pay page -->
								<a
									href={i.payUrl}
									target="_blank"
									rel="noopener noreferrer"
									class="rounded-control bg-accent px-3 py-1.5 text-sm font-medium text-accent-fg hover:opacity-90"
								>
									Pay
								</a>
								<!-- eslint-enable svelte/no-navigation-without-resolve -->
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
