<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus, Search } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { TRACKING, label, qty } from '#lib/inventory/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let stockOpen = $state(false);
</script>

<svelte:head>
	<title>Inventory · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Inventory</h1>
			<p class="mt-1 text-sm text-fg-subtle">What's in stock, where, and what's running low.</p>
		</div>
		{#if data.canConfigure}
			<div class="ml-auto flex gap-2">
				<Button href={resolve('/(app)/settings/inventory')}>Locations</Button>
				<Button variant="primary" onclick={() => (stockOpen = true)}>
					<Plus size={16} /> Stock a product
				</Button>
			</div>
		{/if}
	</div>

	<form method="GET" class="flex flex-wrap items-center gap-2" role="search">
		<label for="location" class="sr-only">Location</label>
		<Select
			id="location"
			name="location"
			value={data.location}
			options={[{ value: '', label: 'Every location' }, ...data.locations]}
		/>
		<label for="q" class="sr-only">Search items</label>
		<Input id="q" name="q" value={data.q} placeholder="SKU or name" class="w-56" />
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="low" value="1" checked={data.low} /> Running low
		</label>
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.items.length === 0}
		<EmptyState
			title="Nothing stocked"
			description={data.canConfigure
				? 'Stock a product from the catalog, then receive it into a location.'
				: 'Items your organization stocks show here.'}
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Items">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Item</th>
						<th scope="col" class="px-4 py-2.5">Tracked</th>
						<th scope="col" class="px-4 py-2.5 text-right">On hand</th>
						<th scope="col" class="px-4 py-2.5 text-right">Allocated</th>
						<th scope="col" class="px-4 py-2.5 text-right">Available</th>
					</tr>
				</thead>
				<tbody>
					{#each data.items as i (i.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/inventory/items/[id]', { id: i.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{i.name}
								</a>
								<span class="block font-mono text-xs text-fg-subtle">{i.sku}</span>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{label(TRACKING, i.tracking)}</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">{qty(i.onHand)}</td>
							<td class="px-4 py-2.5 text-right font-mono text-fg-muted tabular-nums">
								{qty(i.allocated)}
							</td>
							<td class="px-4 py-2.5 text-right font-mono font-semibold tabular-nums">
								{qty(i.available)}
								{#if i.low}<Badge tone="warning">Low</Badge>{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Dialog bind:open={stockOpen} title="Stock a product">
	<form
		id="stock-form"
		method="POST"
		action="?/stock"
		class="flex flex-col gap-4"
		use:enhance={() =>
			async ({ result, update }) => {
				if (result.type === 'failure') {
					const failed = result.data as { message?: string } | undefined;
					toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
				}
				await update({ reset: false });
			}}
	>
		<Field label="Product" for="stock-product" hint="Products from the catalog, not services">
			<Select id="stock-product" name="productId" required options={data.products} />
		</Field>
		{#if data.providers.length > 1}
			<Field label="Stocked by" for="stock-org">
				<Select id="stock-org" name="orgId" options={data.providers} />
			</Field>
		{:else}
			<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
		{/if}
		<Field label="Tracked" for="stock-tracking">
			<Select
				id="stock-tracking"
				name="tracking"
				options={TRACKING.map((t) => ({ value: t.value, label: t.label }))}
			/>
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (stockOpen = false)}>Cancel</Button>
		<Button type="submit" form="stock-form" variant="primary">Stock it</Button>
	{/snippet}
</Dialog>
