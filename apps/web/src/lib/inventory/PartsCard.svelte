<!-- Parts on a ticket or project: set aside from stock, then used or put back. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Package, Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { qty } from '#lib/inventory/labels.ts';

	type Option = { value: string; label: string; orgId: string };
	let {
		parts
	}: {
		parts: {
			allocations: components['schemas']['InventoryAllocation'][];
			items: { id: string; orgId: string; label: string; tracking: string }[];
			locations: Option[];
			canWrite: boolean;
		};
	} = $props();

	let open = $state(false);
	let itemId = $state('');
	let item = $derived(parts.items.find((i) => i.id === itemId));
	let locations = $derived(parts.locations.filter((l) => !item || l.orgId === item.orgId));
	let active = $derived(parts.allocations.filter((a) => a.status !== 'released'));

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const ok = result.data as { message?: string } | undefined;
				toast.push({ title: ok?.message ?? 'Saved.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<Card title="Parts">
	{#snippet actions()}
		{#if parts.canWrite && parts.items.length > 0}
			<Button
				size="sm"
				onclick={() => {
					itemId = parts.items[0]?.id ?? '';
					open = true;
				}}
			>
				<Plus size={14} /> Add part
			</Button>
		{/if}
	{/snippet}
	{#if active.length === 0}
		<p class="flex items-center gap-2 text-sm text-fg-muted">
			<Package size={15} aria-hidden="true" /> No parts yet.
		</p>
	{:else}
		<ul class="flex flex-col gap-2 text-sm" aria-label="Parts">
			{#each active as a (a.id)}
				<li class="flex flex-col gap-1">
					<span class="flex flex-wrap items-center gap-2">
						<a
							href={resolve('/(app)/inventory/items/[id]', { id: a.itemId })}
							class="font-semibold text-fg hover:text-accent"
						>
							{qty(a.quantity)} × {a.name}
						</a>
						<Badge tone={a.status === 'consumed' ? 'neutral' : 'info'}>
							{a.status === 'consumed' ? 'Used' : 'Set aside'}
						</Badge>
					</span>
					<span class="text-xs text-fg-muted">
						From {a.locationName}{a.lot ? ` · lot ${a.lot}` : ''}{a.serials.length
							? ` · ${a.serials.join(', ')}`
							: ''}
					</span>
					{#if parts.canWrite && a.status === 'allocated'}
						<form method="POST" action="?/closePart" use:enhance={done()} class="flex gap-1">
							<input type="hidden" name="allocationId" value={a.id} />
							<Button type="submit" size="sm" name="to" value="consume">Used</Button>
							<Button type="submit" size="sm" variant="ghost" name="to" value="release">
								Put back
							</Button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</Card>

<Dialog bind:open title="Add a part">
	<form
		id="part-form"
		method="POST"
		action="?/addPart"
		class="flex flex-col gap-4"
		use:enhance={done(() => (open = false))}
	>
		<Field label="Item" for="part-item">
			<Select
				id="part-item"
				name="itemId"
				bind:value={itemId}
				options={parts.items.map((i) => ({ value: i.id, label: i.label }))}
			/>
		</Field>
		<Field label="From" for="part-location">
			<Select id="part-location" name="locationId" options={locations} />
		</Field>
		{#if item?.tracking === 'serial'}
			<Field label="Serial numbers" for="part-serials" hint="One per line">
				<textarea
					id="part-serials"
					name="serials"
					rows="3"
					required
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 font-mono text-sm text-fg"
				></textarea>
			</Field>
		{:else}
			<Field label="Quantity" for="part-quantity">
				<Input id="part-quantity" name="quantity" inputmode="decimal" required value="1" />
			</Field>
		{/if}
		{#if item?.tracking === 'lot'}
			<Field label="Lot" for="part-lot">
				<Input id="part-lot" name="lot" required maxlength={100} />
			</Field>
		{/if}
		<Field label="Note" for="part-note">
			<Input id="part-note" name="note" maxlength={1000} />
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="consume" /> Used already (not just set aside)
		</label>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="part-form" variant="primary">Add</Button>
	{/snippet}
</Dialog>
