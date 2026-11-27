<script lang="ts">
	import { resolve } from '$app/paths';
	import { Search } from '@lucide/svelte';
	import { Badge, Button, EmptyState, Input, Select } from '@usehuxley/ui';
	import { ORDER_STATUSES, label, money, orderTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Orders · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Orders</h1>
		<p class="mt-1 text-sm text-fg-subtle">Accepted quotes, being delivered and handed off.</p>
	</div>

	<form method="GET" class="flex flex-wrap items-center gap-2" role="search">
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[{ value: '', label: 'Any status' }, ...ORDER_STATUSES.map((s) => ({ ...s }))]}
		/>
		<label for="q" class="sr-only">Search orders</label>
		<Input id="q" name="q" value={data.q} placeholder="Title, client or SO number" class="w-64" />
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.orders.length === 0}
		<EmptyState
			title="No orders"
			description="When a client accepts a quote, its order shows here to deliver."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Sales orders">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Order</th>
						<th scope="col" class="px-4 py-2.5">Client</th>
						<th scope="col" class="px-4 py-2.5">Status</th>
						<th scope="col" class="px-4 py-2.5 text-right">One-time</th>
						<th scope="col" class="px-4 py-2.5 text-right">Monthly</th>
						<th scope="col" class="px-4 py-2.5 text-right">Ordered</th>
					</tr>
				</thead>
				<tbody>
					{#each data.orders as o (o.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/sales/orders/[id]', { id: o.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{o.title}
								</a>
								<span class="block font-mono text-xs text-fg-subtle">SO-{o.number}</span>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{o.orgName}</td>
							<td class="px-4 py-2.5">
								<Badge tone={orderTone(o.status)}>{label(ORDER_STATUSES, o.status)}</Badge>
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.oneTime, o.currency)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.monthly, o.currency)}
							</td>
							<td class="px-4 py-2.5 text-right whitespace-nowrap text-fg-muted">
								{new Date(o.createdAt).toLocaleDateString()}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
