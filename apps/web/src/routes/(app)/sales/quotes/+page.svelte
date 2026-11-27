<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus, Search } from '@lucide/svelte';
	import { Badge, Button, EmptyState, Input, Select } from '@usehuxley/ui';
	import { QUOTE_STATUSES, label, money, quoteRef, quoteTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Quotes · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Quotes</h1>
			<p class="mt-1 text-sm text-fg-subtle">What's been offered, and what clients said.</p>
		</div>
		{#if data.canWrite}
			<div class="ml-auto">
				<Button href={resolve('/(app)/sales/quotes/new')} variant="primary">
					<Plus size={16} /> New quote
				</Button>
			</div>
		{/if}
	</div>

	<form method="GET" class="flex flex-wrap items-center gap-2" role="search">
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[{ value: '', label: 'Any status' }, ...QUOTE_STATUSES.map((s) => ({ ...s }))]}
		/>
		<label for="q" class="sr-only">Search quotes</label>
		<Input id="q" name="q" value={data.q} placeholder="Title, client or number" class="w-64" />
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.quotes.length === 0}
		<EmptyState title="No quotes" description="Quotes you draft for clients show here." />
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Quotes">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Quote</th>
						<th scope="col" class="px-4 py-2.5">Client</th>
						<th scope="col" class="px-4 py-2.5">Status</th>
						<th scope="col" class="px-4 py-2.5 text-right">One-time</th>
						<th scope="col" class="px-4 py-2.5 text-right">Monthly</th>
						<th scope="col" class="px-4 py-2.5 text-right">Margin</th>
						<th scope="col" class="px-4 py-2.5 text-right">Updated</th>
					</tr>
				</thead>
				<tbody>
					{#each data.quotes as q (q.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/sales/quotes/[id]', { id: q.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{q.title}
								</a>
								<span class="block font-mono text-xs text-fg-subtle">
									{quoteRef(q.number)} · rev {q.revision}
								</span>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{q.orgName}</td>
							<td class="px-4 py-2.5">
								<Badge tone={quoteTone(q.status)}>{label(QUOTE_STATUSES, q.status)}</Badge>
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(q.totals.oneTime, q.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(q.totals.monthly, q.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{q.totals.marginPct != null ? `${q.totals.marginPct}%` : ''}
							</td>
							<td class="px-4 py-2.5 text-right whitespace-nowrap text-fg-muted">
								{new Date(q.updatedAt).toLocaleDateString()}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
