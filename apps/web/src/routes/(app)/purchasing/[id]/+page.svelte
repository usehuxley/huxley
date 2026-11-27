<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Download, PackageCheck, Plus, Send, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast, tray } from '@usehuxley/ui';
	import { qty } from '#lib/inventory/labels.ts';
	import { PO_STATUSES, money, poRef, poTone } from '#lib/inventory/purchasing.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let o = $derived(data.order);
	let draft = $derived(o.status === 'draft' && data.canWrite);
	const statusLabel = (s: string) => PO_STATUSES.find((x) => x.value === s)?.label ?? s;

	$effect(() => {
		tray.open({
			key: `po:${o.id}`,
			kind: 'other',
			href: `/purchasing/${o.id}`,
			title: poRef(o.number),
			subtitle: o.vendorName
		});
	});

	type Row = {
		key: number;
		itemId: string | null;
		sku: string;
		name: string;
		quantity: string;
		unitCost: string;
	};
	let seq = 0;
	let vendorId = $state('');
	let shipTo = $state('');
	let expectedOn = $state('');
	let vendorReference = $state('');
	let notes = $state('');
	let rows = $state<Row[]>([]);
	let dirty = $state(false);

	$effect(() => {
		const saved = data.order;
		vendorId = saved.vendorId;
		shipTo = saved.shipTo ?? '';
		expectedOn = saved.expectedOn ?? '';
		vendorReference = saved.vendorReference ?? '';
		notes = saved.notes ?? '';
		rows = saved.lines.map((l) => ({
			key: ++seq,
			itemId: l.itemId ?? null,
			sku: l.sku ?? '',
			name: l.name,
			quantity: l.quantity,
			unitCost: l.unitCost
		}));
		dirty = false;
	});

	const change = () => (dirty = true);
	const n = (s: string) => (Number.isFinite(Number(s)) ? Number(s) : 0);
	let total = $derived(
		rows.reduce((t, r) => t + Math.round(n(r.quantity) * n(r.unitCost) * 100) / 100, 0)
	);

	function addItem(id: string) {
		const i = data.items.find((x) => x.id === id);
		if (!i) return;
		rows.push({ key: ++seq, itemId: i.id, sku: i.sku, name: i.name, quantity: '1', unitCost: '' });
		change();
	}

	let doc = $derived(
		JSON.stringify({
			vendorId,
			shipTo: shipTo || null,
			expectedOn: expectedOn || null,
			vendorReference: vendorReference || null,
			notes: notes || null,
			version: o.version,
			lines: rows.map((r) => ({
				itemId: r.itemId,
				sku: r.itemId ? null : r.sku || null,
				name: r.itemId ? null : r.name,
				quantity: r.quantity || '0',
				unitCost: r.unitCost === '' ? null : r.unitCost
			}))
		})
	);

	let receiveOpen = $state(false);
	let rejectOpen = $state(false);
	let orderOpen = $state(false);
	let outstanding = $derived(o.lines.filter((l) => Number(l.received) < Number(l.quantity)));

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const said = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (said as { message?: string } | undefined)?.message;
			if (result.type === 'success') {
				toast.push({ title: message ?? 'Done.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				toast.push({ title: message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const cell =
		'h-8 w-full rounded-control border border-border bg-surface px-2 text-sm text-fg disabled:border-transparent disabled:bg-transparent';
</script>

{#snippet stepButton(
	step: string,
	text: string,
	variant: 'primary' | 'secondary' | 'danger' | 'ghost' = 'secondary'
)}
	<form method="POST" action="?/step" use:enhance={done()}>
		<input type="hidden" name="version" value={o.version} />
		<Button
			type="submit"
			name="step"
			value={step}
			{variant}
			disabled={dirty}
			title={dirty ? 'Save first' : undefined}
		>
			{text}
		</Button>
	</form>
{/snippet}

<svelte:head>
	<title>{poRef(o.number)} · Purchasing · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/purchasing')} class="text-sm text-fg-muted hover:text-fg">
			← Purchasing
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<p class="font-mono text-[12.5px] text-fg-subtle">
					{poRef(o.number)}{o.createdByName ? ` · by ${o.createdByName}` : ''}
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">{o.vendorName}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					<Badge tone={poTone(o.status)}>{statusLabel(o.status)}</Badge>
					{#if o.approvedByName}<Badge>Approved by {o.approvedByName}</Badge>{/if}
					{#if dirty}<Badge tone="warning">Unsaved changes</Badge>{/if}
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button href="{resolve('/(app)/purchasing/[id]', { id: o.id })}/pdf" variant="secondary">
					<Download size={16} /> PDF
				</Button>
				{#if draft}
					<form method="POST" action="?/save" use:enhance={done()}>
						<input type="hidden" name="doc" value={doc} />
						<Button type="submit" variant={dirty ? 'primary' : 'secondary'}>Save</Button>
					</form>
					{@render stepButton('submit', 'Submit', 'primary')}
				{/if}
				{#if o.status === 'submitted' && data.canApprove}
					{@render stepButton('approve', 'Approve', 'primary')}
					<Button onclick={() => (rejectOpen = true)}>Send back</Button>
				{/if}
				{#if o.status === 'approved' && data.canWrite}
					<Button variant="primary" onclick={() => (orderOpen = true)}>
						<Send size={16} /> Mark ordered
					</Button>
				{/if}
				{#if ['approved', 'ordered', 'partially_received'].includes(o.status) && data.canWrite}
					<Button onclick={() => (receiveOpen = true)}><PackageCheck size={16} /> Receive</Button>
				{/if}
				{#if o.status === 'partially_received' && data.canWrite}
					{@render stepButton('close', 'Close (the rest isn’t coming)', 'ghost')}
				{/if}
				{#if ['draft', 'submitted', 'approved', 'ordered'].includes(o.status) && data.canWrite}
					{@render stepButton('cancel', 'Cancel order', 'ghost')}
				{/if}
			</div>
		</div>
		{#if o.rejection && o.status === 'draft'}
			<p class="mt-3 rounded-control bg-warning-soft px-3 py-2 text-sm text-fg">
				Sent back: {o.rejection}
			</p>
		{/if}
	</div>

	<Card>
		<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
			<Field label="Vendor" for="po-vendor">
				<Select
					id="po-vendor"
					bind:value={vendorId}
					onchange={change}
					disabled={!draft}
					options={data.vendors}
				/>
			</Field>
			<Field label="Deliver to" for="po-ship">
				<Select
					id="po-ship"
					bind:value={shipTo}
					onchange={change}
					disabled={!draft}
					options={[{ value: '', label: 'Not set' }, ...data.locations]}
				/>
			</Field>
			<Field label="Expected" for="po-expected">
				<Input
					id="po-expected"
					type="date"
					bind:value={expectedOn}
					oninput={change}
					disabled={!draft}
				/>
			</Field>
			<Field label="Their reference" for="po-ref">
				<Input
					id="po-ref"
					bind:value={vendorReference}
					oninput={change}
					disabled={!draft}
					maxlength={200}
				/>
			</Field>
		</div>
	</Card>

	<section class="overflow-x-auto rounded-card border border-border bg-surface" aria-label="Lines">
		<table class="w-full min-w-[720px] text-left text-sm">
			<thead class="border-b border-border text-xs font-bold text-fg-subtle">
				<tr>
					<th scope="col" class="px-3 py-2.5">Item</th>
					<th scope="col" class="w-24 px-3 py-2.5 text-right">Quantity</th>
					<th scope="col" class="w-28 px-3 py-2.5 text-right">Unit cost</th>
					<th scope="col" class="w-28 px-3 py-2.5 text-right">Total</th>
					<th scope="col" class="w-28 px-3 py-2.5 text-right">Received</th>
					<th scope="col" class="w-10 px-3 py-2.5"><span class="sr-only">Remove</span></th>
				</tr>
			</thead>
			<tbody>
				{#each rows as r, i (r.key)}
					{@const saved = o.lines[i]}
					<tr class="border-b border-border last:border-0">
						<td class="px-3 py-1.5">
							{#if r.itemId}
								<span class="font-semibold text-fg">{r.name}</span>
								<span class="block font-mono text-[11.5px] text-fg-subtle">{r.sku}</span>
							{:else}
								<input
									class={cell}
									aria-label="Item name"
									bind:value={r.name}
									oninput={change}
									disabled={!draft}
									placeholder="What it is"
								/>
							{/if}
						</td>
						<td class="px-3 py-1.5">
							<input
								class="{cell} text-right tabular-nums"
								aria-label="Quantity"
								inputmode="decimal"
								bind:value={r.quantity}
								oninput={change}
								disabled={!draft}
							/>
						</td>
						<td class="px-3 py-1.5">
							<input
								class="{cell} text-right tabular-nums"
								aria-label="Unit cost"
								inputmode="decimal"
								bind:value={r.unitCost}
								oninput={change}
								disabled={!draft}
								placeholder="From catalog"
							/>
						</td>
						<td class="px-3 py-2.5 text-right font-mono tabular-nums">
							{money(n(r.quantity) * n(r.unitCost), o.currency)}
						</td>
						<td class="px-3 py-2.5 text-right font-mono text-fg-muted tabular-nums">
							{!dirty && saved ? `${qty(saved.received)} / ${qty(saved.quantity)}` : ''}
						</td>
						<td class="px-3 py-1.5">
							{#if draft}
								<button
									type="button"
									class="grid size-8 place-items-center rounded text-fg-subtle hover:bg-danger-soft hover:text-danger"
									aria-label="Remove {r.name || 'line'}"
									onclick={() => (rows.splice(i, 1), change())}
								>
									<Trash2 size={15} />
								</button>
							{/if}
						</td>
					</tr>
				{:else}
					<tr>
						<td colspan="6" class="px-4 py-8 text-center text-sm text-fg-subtle">
							Nothing on the order yet.
						</td>
					</tr>
				{/each}
			</tbody>
			<tfoot>
				<tr class="border-t border-border">
					<th scope="row" colspan="3" class="px-3 py-2.5 text-right font-semibold text-fg">
						Total
					</th>
					<td class="px-3 py-2.5 text-right font-mono font-semibold tabular-nums">
						{money(dirty ? total : o.total, o.currency)}
					</td>
					<td colspan="2"></td>
				</tr>
			</tfoot>
		</table>
		{#if draft}
			<div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2.5">
				<label for="add-item" class="sr-only">Add a stocked item</label>
				<select
					id="add-item"
					class="h-8 max-w-xs rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
					value=""
					onchange={(e) => {
						addItem(e.currentTarget.value);
						e.currentTarget.value = '';
					}}
				>
					<option value="">Add a stocked item…</option>
					{#each data.items as i (i.id)}<option value={i.id}>{i.name} · {i.sku}</option>{/each}
				</select>
				<Button
					size="sm"
					onclick={() => (
						rows.push({
							key: ++seq,
							itemId: null,
							sku: '',
							name: '',
							quantity: '1',
							unitCost: '0'
						}),
						change()
					)}
				>
					<Plus size={14} /> Line
				</Button>
			</div>
		{/if}
	</section>

	{#if draft || o.notes}
		<Card title="Notes for the vendor">
			<textarea
				aria-label="Notes for the vendor"
				rows="3"
				class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				bind:value={notes}
				oninput={change}
				disabled={!draft}></textarea>
		</Card>
	{/if}

	{#if data.receipts.length > 0}
		<Card title="Deliveries">
			<ol class="flex flex-col divide-y divide-border text-sm" aria-label="Deliveries">
				{#each data.receipts as r (r.id)}
					<li class="flex flex-wrap justify-between gap-2 py-2">
						<span>
							<span class="font-semibold text-fg">{qty(r.quantity)} × {r.name}</span>
							{#if r.locationName}<span class="text-fg-muted">into {r.locationName}</span>{/if}
							{#if r.lot}<span class="font-mono text-xs text-fg-subtle">lot {r.lot}</span>{/if}
							{#if r.serials.length}<span class="font-mono text-xs text-fg-subtle">
									{r.serials.join(', ')}
								</span>{/if}
						</span>
						<span class="text-xs text-fg-subtle">
							{r.receivedByName ?? ''} · {new Date(r.receivedAt).toLocaleString()}
						</span>
					</li>
				{/each}
			</ol>
		</Card>
	{/if}
</div>

<Dialog bind:open={receiveOpen} title="Receive a delivery">
	<form
		id="receive-form"
		method="POST"
		action="?/receive"
		class="flex flex-col gap-4"
		use:enhance={done(() => (receiveOpen = false))}
	>
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Put into" for="receive-location">
				<Select
					id="receive-location"
					name="locationId"
					value={o.shipTo ?? data.locations[0]?.value ?? ''}
					options={data.locations}
				/>
			</Field>
			<Field label="Delivery note" for="receive-ref">
				<Input id="receive-ref" name="reference" maxlength={200} />
			</Field>
		</div>
		{#each outstanding as l (l.id)}
			<fieldset class="flex flex-col gap-2 rounded-control border border-border p-3">
				<legend class="px-1 text-sm font-semibold text-fg">
					{l.name} · {qty(Number(l.quantity) - Number(l.received))} to come
				</legend>
				<input type="hidden" name="lineId" value={l.id} />
				{#if l.tracking === 'serial'}
					<Field
						label="Serial numbers"
						for="serials-{l.id}"
						hint="One per line; leave empty if none came"
					>
						<textarea
							id="serials-{l.id}"
							name="serials-{l.id}"
							rows="2"
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 font-mono text-sm text-fg"
						></textarea>
					</Field>
				{:else}
					<Field label="Quantity arrived" for="quantity-{l.id}">
						<Input
							id="quantity-{l.id}"
							name="quantity-{l.id}"
							inputmode="decimal"
							value={String(Number(l.quantity) - Number(l.received))}
						/>
					</Field>
				{/if}
				{#if l.tracking === 'lot'}
					<Field label="Lot" for="lot-{l.id}">
						<Input id="lot-{l.id}" name="lot-{l.id}" maxlength={100} />
					</Field>
				{/if}
			</fieldset>
		{/each}
	</form>
	{#snippet footer()}
		<Button onclick={() => (receiveOpen = false)}>Cancel</Button>
		<Button type="submit" form="receive-form" variant="primary">Receive</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={rejectOpen} title="Send it back">
	<form
		id="reject-form"
		method="POST"
		action="?/step"
		class="flex flex-col gap-4"
		use:enhance={done(() => (rejectOpen = false))}
	>
		<input type="hidden" name="version" value={o.version} />
		<input type="hidden" name="step" value="reject" />
		<Field label="What should change" for="reject-why">
			<Input id="reject-why" name="comment" required maxlength={2000} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (rejectOpen = false)}>Cancel</Button>
		<Button type="submit" form="reject-form" variant="danger">Send back</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={orderOpen} title="Mark ordered">
	<form
		id="order-form"
		method="POST"
		action="?/step"
		class="flex flex-col gap-4"
		use:enhance={done(() => (orderOpen = false))}
	>
		<input type="hidden" name="version" value={o.version} />
		<input type="hidden" name="step" value="order" />
		<p class="text-sm text-fg-muted">
			Send the PDF to {o.vendorEmail ?? 'the vendor'}, then record it here.
		</p>
		<Field label="Their order reference" for="order-ref">
			<Input id="order-ref" name="comment" maxlength={200} value={o.vendorReference ?? ''} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (orderOpen = false)}>Cancel</Button>
		<Button type="submit" form="order-form" variant="primary">Mark ordered</Button>
	{/snippet}
</Dialog>
