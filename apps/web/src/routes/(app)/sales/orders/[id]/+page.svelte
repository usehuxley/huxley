<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Boxes, FileSignature, FolderKanban, PackageCheck, ShoppingCart } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast, tray } from '@usehuxley/ui';
	import { BILLINGS, ORDER_STATUSES, label, money, orderTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let o = $derived(data.order);
	let open = $derived(o.status === 'open' && data.can.write);
	let stocked = $derived(o.lines.some((l) => l.itemId));
	let recurring = $derived(o.lines.some((l) => l.billing !== 'one_time'));
	const n = (s: string) => Number(s);

	$effect(() => {
		tray.open({
			key: `so:${o.id}`,
			kind: 'other',
			href: `/sales/orders/${o.id}`,
			title: o.title,
			subtitle: `SO-${o.number} · ${o.orgName}`
		});
	});

	let agreementOpen = $state(false);
	let projectOpen = $state(false);
	let cancelOpen = $state(false);

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
	const lineState = (l: (typeof o.lines)[number]) => {
		if (n(l.fulfilled) >= n(l.quantity)) return { text: 'Delivered', tone: 'success' as const };
		if (l.billing !== 'one_time') return { text: 'For the agreement', tone: 'neutral' as const };
		if (!l.itemId) return { text: 'Delivered with the order', tone: 'neutral' as const };
		if (n(l.allocated) + n(l.fulfilled) >= n(l.quantity))
			return { text: 'Ready', tone: 'info' as const };
		if (n(l.allocated) + n(l.onOrder) + n(l.fulfilled) >= n(l.quantity))
			return { text: 'On order', tone: 'warning' as const };
		return { text: 'Short', tone: 'danger' as const };
	};
</script>

{#snippet act(
	action: string,
	text: string,
	Icon: typeof Boxes,
	variant: 'primary' | 'secondary' = 'secondary'
)}
	<form method="POST" action="?/{action}" use:enhance={done()}>
		<input type="hidden" name="version" value={o.version} />
		<Button type="submit" {variant}><Icon size={16} /> {text}</Button>
	</form>
{/snippet}

<svelte:head>
	<title>SO-{o.number} · Orders · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/sales/orders')} class="text-sm text-fg-muted hover:text-fg">
			← Orders
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<p class="font-mono text-[12.5px] text-fg-subtle">
					SO-{o.number} ·
					<a
						href={resolve('/(app)/clients/[id]', { id: o.orgId })}
						class="hover:text-fg hover:underline"
					>
						{o.orgName}
					</a>
					{#if o.quoteId}
						· <a
							href={resolve('/(app)/sales/quotes/[id]', { id: o.quoteId })}
							class="hover:text-fg hover:underline"
						>
							Q-{o.quoteNumber}
						</a>
					{/if}
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">{o.title}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					<Badge tone={orderTone(o.status)}>{label(ORDER_STATUSES, o.status)}</Badge>
					{#if o.agreementId}
						<a href="{resolve('/(app)/clients/[id]', { id: o.orgId })}?tab=agreements">
							<Badge>Agreement: {o.agreementName}</Badge>
						</a>
					{/if}
					{#if o.projectId}
						<a href={resolve('/(app)/projects/[id]', { id: o.projectId })}>
							<Badge>Project: {o.projectName}</Badge>
						</a>
					{/if}
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				{#if open && stocked && data.can.stock}{@render act(
						'allocate',
						'Set aside stock',
						Boxes
					)}{/if}
				{#if open && stocked && data.can.buy}{@render act(
						'purchase',
						'Order what’s short',
						ShoppingCart
					)}{/if}
				{#if open}{@render act('fulfil', 'Deliver what’s ready', PackageCheck, 'primary')}{/if}
				{#if open && recurring && !o.agreementId && data.can.agreement}
					<Button onclick={() => (agreementOpen = true)}>
						<FileSignature size={16} /> Make the agreement
					</Button>
				{/if}
				{#if data.can.write && data.can.project && !o.projectId && (o.status === 'open' || o.status === 'fulfilled')}
					<Button onclick={() => (projectOpen = true)}>
						<FolderKanban size={16} /> Start a project
					</Button>
				{/if}
				{#if o.status === 'fulfilled' && data.can.write}
					<form method="POST" action="?/finish" use:enhance={done()}>
						<input type="hidden" name="version" value={o.version} />
						<input type="hidden" name="how" value="close" />
						<Button type="submit">Close</Button>
					</form>
				{/if}
				{#if open}
					<Button variant="ghost" onclick={() => (cancelOpen = true)}>Cancel order</Button>
				{/if}
			</div>
		</div>
	</div>

	<section class="overflow-x-auto rounded-card border border-border bg-surface" aria-label="Lines">
		<table class="w-full min-w-[760px] text-left text-sm">
			<thead class="border-b border-border text-xs font-bold text-fg-subtle">
				<tr>
					<th scope="col" class="px-4 py-2.5">Item</th>
					<th scope="col" class="px-4 py-2.5 text-right">Quantity</th>
					<th scope="col" class="px-4 py-2.5 text-right">Total</th>
					<th scope="col" class="px-4 py-2.5 text-right">Set aside</th>
					<th scope="col" class="px-4 py-2.5 text-right">On order</th>
					<th scope="col" class="px-4 py-2.5 text-right">Delivered</th>
					<th scope="col" class="px-4 py-2.5">State</th>
				</tr>
			</thead>
			<tbody>
				{#each o.lines as l (l.id)}
					{@const s = lineState(l)}
					<tr class="border-b border-border last:border-0">
						<td class="px-4 py-2.5">
							<span class="font-semibold text-fg">{l.name}</span>
							<span class="block text-xs text-fg-subtle">
								{[l.sku, label(BILLINGS, l.billing).toLowerCase()].filter(Boolean).join(' · ')}
							</span>
						</td>
						<td class="px-4 py-2.5 text-right font-mono tabular-nums">{n(l.quantity)} {l.unit}</td>
						<td class="px-4 py-2.5 text-right font-mono tabular-nums">
							{money(l.total, o.currency)}
						</td>
						<td class="px-4 py-2.5 text-right font-mono text-fg-muted tabular-nums">
							{l.itemId ? n(l.allocated) : ''}
						</td>
						<td class="px-4 py-2.5 text-right font-mono text-fg-muted tabular-nums">
							{l.itemId ? n(l.onOrder) : ''}
						</td>
						<td class="px-4 py-2.5 text-right font-mono tabular-nums">{n(l.fulfilled)}</td>
						<td class="px-4 py-2.5"><Badge tone={s.tone}>{s.text}</Badge></td>
					</tr>
				{/each}
			</tbody>
			<tfoot>
				<tr class="border-t border-border">
					<th scope="row" colspan="2" class="px-4 py-2.5 text-right font-semibold text-fg">
						One-time · monthly
					</th>
					<td class="px-4 py-2.5 text-right font-mono font-semibold tabular-nums" colspan="2">
						{money(o.oneTime, o.currency)} · {money(o.monthly, o.currency)}
					</td>
					<td colspan="3"></td>
				</tr>
			</tfoot>
		</table>
	</section>

	{#if o.notes}
		<Card title="Notes"><p class="text-sm whitespace-pre-wrap text-fg">{o.notes}</p></Card>
	{/if}
</div>

<Dialog bind:open={agreementOpen} title="Make the agreement">
	<form
		id="agreement-form"
		method="POST"
		action="?/agreement"
		class="flex flex-col gap-4"
		use:enhance={done(() => (agreementOpen = false))}
	>
		<p class="text-sm text-fg-muted">
			The recurring lines become the agreement's billing lines. Coverage and rates can be added on
			the agreement.
		</p>
		<Field label="Name" for="agreement-name">
			<Input id="agreement-name" name="name" value={o.title} maxlength={200} />
		</Field>
		<Field label="Starts" for="agreement-start">
			<Input id="agreement-start" name="startsOn" type="date" />
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="autoRenew" /> Renews automatically
		</label>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="activate" /> Active now (not a draft)
		</label>
	</form>
	{#snippet footer()}
		<Button onclick={() => (agreementOpen = false)}>Cancel</Button>
		<Button type="submit" form="agreement-form" variant="primary">Make it</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={projectOpen} title="Start a project">
	<form
		id="project-form"
		method="POST"
		action="?/project"
		class="flex flex-col gap-4"
		use:enhance={done(() => (projectOpen = false))}
	>
		<Field label="Name" for="project-name">
			<Input id="project-name" name="name" value={o.title} maxlength={200} />
		</Field>
		<Field label="From template" for="project-template">
			<Select
				id="project-template"
				name="templateId"
				options={[{ value: '', label: 'No template' }, ...data.templates]}
			/>
		</Field>
		<Field label="Starts" for="project-start">
			<Input id="project-start" name="startOn" type="date" />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (projectOpen = false)}>Cancel</Button>
		<Button type="submit" form="project-form" variant="primary">Start it</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={cancelOpen} title="Cancel the order?">
	<form
		id="cancel-form"
		method="POST"
		action="?/finish"
		class="flex flex-col gap-4"
		use:enhance={done(() => (cancelOpen = false))}
	>
		<input type="hidden" name="version" value={o.version} />
		<input type="hidden" name="how" value="cancel" />
		<p class="text-sm text-fg-muted">
			Stock set aside for it is given back. Purchase orders made for it stay as they are.
		</p>
		<Field label="Why" for="cancel-why">
			<Input id="cancel-why" name="note" maxlength={4000} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (cancelOpen = false)}>Keep it</Button>
		<Button type="submit" form="cancel-form" variant="danger">Cancel order</Button>
	{/snippet}
</Dialog>
