<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowRightLeft, PackagePlus, SlidersHorizontal } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast, tray } from '@usehuxley/ui';
	import {
		MOVEMENT_KINDS,
		MOVES,
		TRACKING,
		UNIT_STATUSES,
		label,
		qty
	} from '#lib/inventory/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let item = $derived(data.item);

	$effect(() => {
		tray.open({
			key: `item:${item.id}`,
			kind: 'other',
			href: `/inventory/items/${item.id}`,
			title: item.name,
			subtitle: item.sku
		});
	});

	let moveOpen = $state(false);
	let kind = $state<string>('receive');
	let from = $state('');
	let reorderOpen = $state(false);
	let reorderAt = $state<(typeof data.levels)[number] | null>(null);
	let reorderLocation = $state('');

	function move(k: string, at = '') {
		kind = k;
		from = at || data.locations[0]?.value || '';
		moveOpen = true;
	}

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	let serial = $derived(item.tracking === 'serial');
	let inStock = $derived(
		data.units.filter((u) => u.status === 'in_stock' || u.status === 'allocated')
	);
	const where = (m: (typeof data.movements)[number]) =>
		[m.fromLocationName, m.toLocationName].filter(Boolean).join(' → ');
</script>

<svelte:head>
	<title>{item.name} · Inventory · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/inventory')} class="text-sm text-fg-muted hover:text-fg">
			← Inventory
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<p class="font-mono text-[12.5px] text-fg-subtle">
					{item.sku} · {label(TRACKING, item.tracking)}
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">{item.name}</h1>
				<dl class="mt-2 flex flex-wrap gap-4 text-sm" aria-label="Totals">
					<div>
						<dt class="inline text-fg-muted">On hand</dt>
						<dd class="inline font-semibold">{qty(item.onHand)} {item.unit}</dd>
					</div>
					<div>
						<dt class="inline text-fg-muted">Allocated</dt>
						<dd class="inline font-semibold">{qty(item.allocated)}</dd>
					</div>
					<div>
						<dt class="inline text-fg-muted">Available</dt>
						<dd class="inline font-semibold">{qty(item.available)}</dd>
					</div>
					{#if item.low}<Badge tone="warning">Running low</Badge>{/if}
				</dl>
			</div>
			{#if data.canWrite}
				<div class="flex flex-wrap gap-2">
					<Button variant="primary" onclick={() => move('receive')}>
						<PackagePlus size={16} /> Receive
					</Button>
					<Button onclick={() => move('transfer')}><ArrowRightLeft size={16} /> Transfer</Button>
					<Button onclick={() => move('adjust')}><SlidersHorizontal size={16} /> Adjust</Button>
				</div>
			{/if}
		</div>
	</div>

	<Card title="Where it is">
		{#if data.levels.length === 0}
			<p class="text-sm text-fg-muted">None in stock anywhere yet.</p>
		{:else}
			<table class="w-full text-left text-sm" aria-label="Stock by location">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="py-2 pr-3">Location</th>
						<th scope="col" class="py-2 pr-3 text-right">On hand</th>
						<th scope="col" class="py-2 pr-3 text-right">Allocated</th>
						<th scope="col" class="py-2 pr-3 text-right">Available</th>
						<th scope="col" class="py-2 pr-3 text-right">Reorder at</th>
						<th scope="col" class="py-2"><span class="sr-only">Actions</span></th>
					</tr>
				</thead>
				<tbody>
					{#each data.levels as l (l.locationId)}
						<tr class="border-b border-border align-top last:border-0">
							<td class="py-2 pr-3">
								<span class="font-semibold text-fg">{l.locationName}</span>
								{#each l.lots as lot (lot.lot)}
									<span class="block font-mono text-xs text-fg-subtle">
										Lot {lot.lot}: {qty(lot.onHand)}
									</span>
								{/each}
							</td>
							<td class="py-2 pr-3 text-right font-mono tabular-nums">{qty(l.onHand)}</td>
							<td class="py-2 pr-3 text-right font-mono text-fg-muted tabular-nums">
								{qty(l.allocated)}
							</td>
							<td class="py-2 pr-3 text-right font-mono font-semibold tabular-nums">
								{qty(l.available)}
								{#if l.low}<Badge tone="warning">Low</Badge>{/if}
							</td>
							<td class="py-2 pr-3 text-right font-mono text-fg-muted tabular-nums">
								{l.reorderPoint != null ? qty(l.reorderPoint) : '—'}
							</td>
							<td class="py-2 text-right whitespace-nowrap">
								{#if data.canConfigure}
									<Button
										size="sm"
										variant="ghost"
										onclick={() => (
											(reorderAt = l),
											(reorderLocation = l.locationId),
											(reorderOpen = true)
										)}
									>
										Reorder point
									</Button>
								{/if}
								{#if data.canWrite && Number(l.available) > 0}
									<Button size="sm" variant="ghost" onclick={() => move('transfer', l.locationId)}>
										Move
									</Button>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if data.canConfigure}
			<button
				type="button"
				class="mt-3 text-xs text-fg-muted hover:text-fg"
				onclick={() => (
					(reorderAt = null),
					(reorderLocation = data.locations[0]?.value ?? ''),
					(reorderOpen = true)
				)}
			>
				Set a reorder point at another location
			</button>
		{/if}
	</Card>

	{#if serial}
		<Card title="Serial numbers">
			{#if inStock.length === 0}
				<p class="text-sm text-fg-muted">No units in stock.</p>
			{:else}
				<ul class="grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2 lg:grid-cols-3" aria-label="Units">
					{#each inStock as u (u.id)}
						<li class="flex items-center justify-between gap-2">
							<span class="font-mono text-fg">{u.serial}</span>
							<span class="text-xs text-fg-muted">
								{u.locationName}
								<Badge tone={UNIT_STATUSES[u.status]?.tone ?? 'neutral'}>
									{UNIT_STATUSES[u.status]?.label ?? u.status}
								</Badge>
							</span>
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
	{/if}

	<Card title="Movements">
		{#if data.movements.length === 0}
			<p class="text-sm text-fg-muted">Nothing has moved yet.</p>
		{:else}
			<ol class="flex flex-col divide-y divide-border text-sm" aria-label="Movements">
				{#each data.movements as m (m.id)}
					<li class="flex flex-wrap items-baseline justify-between gap-2 py-2">
						<span>
							<span class="font-semibold text-fg">{MOVEMENT_KINDS[m.kind] ?? m.kind}</span>
							<span class="font-mono tabular-nums">{qty(m.quantity)}</span>
							<span class="text-fg-muted">{where(m)}</span>
							{#if m.lot}<span class="font-mono text-xs text-fg-subtle">lot {m.lot}</span>{/if}
							{#if m.serials.length > 0}<span class="font-mono text-xs text-fg-subtle">
									{m.serials.join(', ')}
								</span>{/if}
							{#if m.refType === 'ticket' && m.refId}
								· <a
									href={resolve('/(app)/tickets/[id]', { id: m.refId })}
									class="text-accent hover:underline"
								>
									ticket
								</a>
							{/if}
							{#if m.reference}<span class="text-fg-muted">· {m.reference}</span>{/if}
							{#if m.note}<span class="block text-xs text-fg-muted">{m.note}</span>{/if}
						</span>
						<span class="text-xs text-fg-subtle">
							{m.createdByName ?? 'Workflow'} · {new Date(m.createdAt).toLocaleString()}
						</span>
					</li>
				{/each}
			</ol>
		{/if}
	</Card>
</div>

<Dialog bind:open={moveOpen} title={label(MOVES, kind)}>
	<form
		id="move-form"
		method="POST"
		action="?/move"
		class="flex flex-col gap-4"
		use:enhance={done('Recorded.', () => (moveOpen = false))}
	>
		<Field label="Movement" for="move-kind">
			<Select id="move-kind" name="kind" bind:value={kind} options={MOVES.map((m) => ({ ...m }))} />
		</Field>
		<Field
			label={kind === 'transfer' ? 'From' : kind === 'rma' ? 'Sent back from' : 'Location'}
			for="move-location"
		>
			<Select id="move-location" name="locationId" bind:value={from} options={data.locations} />
		</Field>
		{#if kind === 'transfer'}
			<Field label="To" for="move-to">
				<Select
					id="move-to"
					name="toLocationId"
					options={data.locations.filter((l) => l.value !== from)}
				/>
			</Field>
		{/if}
		{#if serial}
			<Field label="Serial numbers" for="move-serials" hint="One per line">
				<textarea
					id="move-serials"
					name="serials"
					rows="4"
					required
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 font-mono text-sm text-fg"
				></textarea>
			</Field>
			{#if kind === 'adjust'}
				<Field label="Direction" for="move-quantity" hint="Found adds them; lost writes them off">
					<Select
						id="move-quantity"
						name="quantity"
						options={[
							{ value: '', label: 'Found (add)' },
							{ value: '-1', label: 'Lost (write off)' }
						]}
					/>
				</Field>
			{/if}
		{:else}
			<Field
				label="Quantity"
				for="move-quantity"
				hint={kind === 'adjust' ? 'Negative to take some away' : undefined}
			>
				<Input id="move-quantity" name="quantity" inputmode="decimal" required />
			</Field>
		{/if}
		{#if item.tracking === 'lot'}
			<Field label="Lot" for="move-lot">
				<Input id="move-lot" name="lot" required maxlength={100} />
			</Field>
		{/if}
		{#if kind === 'receive'}
			<Field label="Unit cost" for="move-cost">
				<Input id="move-cost" name="unitCost" inputmode="decimal" />
			</Field>
		{/if}
		{#if kind === 'receive' || kind === 'rma'}
			<Field
				label={kind === 'rma' ? 'RMA number' : 'Reference'}
				for="move-reference"
				hint={kind === 'receive' ? 'A delivery note or PO' : undefined}
			>
				<Input id="move-reference" name="reference" maxlength={200} />
			</Field>
		{/if}
		<Field label={kind === 'adjust' ? 'Why' : 'Note'} for="move-note">
			<Input id="move-note" name="note" maxlength={1000} required={kind === 'adjust'} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (moveOpen = false)}>Cancel</Button>
		<Button type="submit" form="move-form" variant="primary">Record</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={reorderOpen} title="Reorder point">
	<form
		id="reorder-form"
		method="POST"
		action="?/reorder"
		class="flex flex-col gap-4"
		use:enhance={done('Saved.', () => (reorderOpen = false))}
	>
		<Field label="Location" for="reorder-location">
			<Select
				id="reorder-location"
				name="locationId"
				bind:value={reorderLocation}
				options={data.locations}
			/>
		</Field>
		<Field
			label="Reorder when available falls to"
			for="reorder-point"
			hint="Leave empty to clear it"
		>
			<Input
				id="reorder-point"
				name="reorderPoint"
				inputmode="decimal"
				value={reorderAt?.reorderPoint ?? ''}
			/>
		</Field>
		<Field label="Order this many" for="reorder-qty">
			<Input
				id="reorder-qty"
				name="reorderQty"
				inputmode="decimal"
				value={reorderAt?.reorderQty ?? ''}
			/>
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (reorderOpen = false)}>Cancel</Button>
		<Button type="submit" form="reorder-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
