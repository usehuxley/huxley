<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus, RefreshCw, Search } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { PO_STATUSES, money, poRef, poTone } from '#lib/inventory/purchasing.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let open = $state(false);
	let orgId = $state('');
	let vendors = $derived(data.vendors.filter((v) => !orgId || v.orgId === orgId));
	const label = (s: string) => PO_STATUSES.find((x) => x.value === s)?.label ?? s;

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const said = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (said as { message?: string } | undefined)?.message;
			if (message) {
				toast.push({ title: message, tone: result.type === 'failure' ? 'danger' : 'success' });
			}
		};
</script>

<svelte:head>
	<title>Purchasing · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Purchasing</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				What's been bought, what's on its way, and what's arrived.
			</p>
		</div>
		{#if data.canWrite}
			<div class="ml-auto flex flex-wrap gap-2">
				<Button href={resolve('/(app)/purchasing/vendors')}>Vendors</Button>
				<form method="POST" action="?/reorder" use:enhance={done}>
					<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
					<Button type="submit" title="Draft orders for what's running low">
						<RefreshCw size={16} /> Reorder low stock
					</Button>
				</form>
				<Button
					variant="primary"
					onclick={() => {
						orgId = data.providers[0]?.value ?? '';
						open = true;
					}}
				>
					<Plus size={16} /> New order
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
			options={[{ value: '', label: 'Any status' }, ...PO_STATUSES.map((s) => ({ ...s }))]}
		/>
		<label for="q" class="sr-only">Search orders</label>
		<Input id="q" name="q" value={data.q} placeholder="PO number or vendor" class="w-56" />
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.orders.length === 0}
		<EmptyState title="No purchase orders" description="Orders to your vendors show here." />
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Purchase orders">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Order</th>
						<th scope="col" class="px-4 py-2.5">Status</th>
						<th scope="col" class="px-4 py-2.5">Deliver to</th>
						<th scope="col" class="px-4 py-2.5 text-right">Total</th>
						<th scope="col" class="px-4 py-2.5 text-right">Updated</th>
					</tr>
				</thead>
				<tbody>
					{#each data.orders as o (o.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/purchasing/[id]', { id: o.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{poRef(o.number)} · {o.vendorName}
								</a>
								{#if o.vendorReference}
									<span class="block text-xs text-fg-subtle">Their ref {o.vendorReference}</span>
								{/if}
							</td>
							<td class="px-4 py-2.5"><Badge tone={poTone(o.status)}>{label(o.status)}</Badge></td>
							<td class="px-4 py-2.5 text-fg-muted">{o.shipToName ?? ''}</td>
							<td class="px-4 py-2.5 text-right font-mono tabular-nums">
								{money(o.total, o.currency)}
							</td>
							<td class="px-4 py-2.5 text-right whitespace-nowrap text-fg-muted">
								{new Date(o.updatedAt).toLocaleDateString()}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Dialog bind:open title="New purchase order">
	<form id="new-po" method="POST" action="?/create" class="flex flex-col gap-4" use:enhance={done}>
		{#if data.providers.length > 1}
			<Field label="Bought by" for="po-org">
				<Select id="po-org" name="orgId" bind:value={orgId} options={data.providers} />
			</Field>
		{:else}
			<input type="hidden" name="orgId" value={orgId} />
		{/if}
		{#if vendors.length === 0}
			<p class="text-sm text-fg-muted">
				Add a vendor first, under <a
					href={resolve('/(app)/purchasing/vendors')}
					class="text-accent hover:underline"
				>
					Vendors
				</a>.
			</p>
		{:else}
			<Field label="Vendor" for="po-vendor">
				<Select
					id="po-vendor"
					name="vendorId"
					options={vendors.map((v) => ({ value: v.value, label: v.label }))}
				/>
			</Field>
		{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="new-po" variant="primary" disabled={vendors.length === 0}>
			Start the order
		</Button>
	{/snippet}
</Dialog>
