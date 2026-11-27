<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Check, Download, Pause, Plus, Send, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, toast, tray } from '@usehuxley/ui';
	import InvoiceReview from '#lib/billing/InvoiceReview.svelte';
	import { INVOICE_STATUSES, invRef, invoiceTone, money } from '#lib/billing/labels.ts';
	import QuoteComments from '#lib/sales/QuoteComments.svelte';
	import { REVIEW_STATUSES, label, reviewTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let i = $derived(data.invoice);
	let draft = $derived(i.status === 'draft' && data.canWrite);
	const statusLabel = (s: string) => INVOICE_STATUSES.find((x) => x.value === s)?.label ?? s;

	$effect(() => {
		tray.open({
			key: `inv:${i.id}`,
			kind: 'other',
			href: `/billing/invoices/${i.id}`,
			title: invRef(i.number),
			subtitle: i.orgName
		});
	});

	type Row = {
		key: number;
		id: string | null;
		description: string;
		quantity: string;
		unit: string;
		unitPrice: string;
		edited: boolean;
		kind: string;
	};
	let seq = 0;
	let rows = $state<Row[]>([]);
	let notes = $state('');
	let dirty = $state(false);
	$effect(() => {
		rows = data.invoice.lines.map((l) => ({
			key: ++seq,
			id: l.id,
			description: l.description,
			quantity: l.quantity,
			unit: l.unit,
			unitPrice: l.unitPrice,
			edited: l.edited,
			kind: l.kind
		}));
		notes = data.invoice.notes ?? '';
		dirty = false;
	});
	const n = (s: string) => (Number.isFinite(Number(s)) ? Number(s) : 0);
	let total = $derived(
		rows.reduce((t, r) => t + Math.round(n(r.quantity) * n(r.unitPrice) * 100) / 100, 0)
	);
	let doc = $derived(
		JSON.stringify({
			version: i.version,
			notes: notes || null,
			lines: rows.map((r) => ({
				id: r.id,
				description: r.description,
				quantity: r.quantity || '0',
				unit: r.unit,
				unitPrice: r.unitPrice || '0'
			}))
		})
	);
	let holdOpen = $state(false);
	let creditOpen = $state(false);

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
	/** For the review and comment panels: a fixed message on success. */
	const said =
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
	let needsReview = $derived(data.review !== null && !data.review.canApprove);
	let commentLines = $derived(i.lines.map((l) => ({ value: l.id, label: l.description })));
	const cell =
		'h-8 w-full rounded-control border border-border bg-surface px-2 text-sm text-fg disabled:border-transparent disabled:bg-transparent';
</script>

<svelte:head>
	<title>{invRef(i.number)} · Invoices · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/billing/invoices')} class="text-sm text-fg-muted hover:text-fg">
			← Invoices
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<p class="font-mono text-[12.5px] text-fg-subtle">
					{invRef(i.number)}{i.periodStart ? ` · ${i.periodStart} to ${i.periodEnd}` : ''}
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">{i.orgName}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					{#if i.kind === 'credit_memo'}<Badge>Credit memo</Badge>{/if}
					<Badge tone={invoiceTone(i.status)}>{statusLabel(i.status)}</Badge>
					{#if i.onHold}<Badge tone="warning">
							On hold{i.holdReason ? `: ${i.holdReason}` : ''}
						</Badge>{/if}
					{#if i.dueOn}<Badge>Due {i.dueOn}</Badge>{/if}
					{#if i.reviewStatus !== 'not_needed'}
						<Badge tone={reviewTone(i.reviewStatus)}>
							{label(REVIEW_STATUSES, i.reviewStatus)}
						</Badge>
					{/if}
					{#if dirty}<Badge tone="warning">Unsaved changes</Badge>{/if}
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button href="{resolve('/(app)/billing/invoices/[id]', { id: i.id })}/pdf">
					<Download size={16} /> PDF
				</Button>
				{#if draft}
					<form method="POST" action="?/save" use:enhance={done()}>
						<input type="hidden" name="doc" value={doc} />
						<Button type="submit" variant={dirty ? 'primary' : 'secondary'}>Save</Button>
					</form>
					{#if i.onHold}
						<form method="POST" action="?/hold" use:enhance={done()}>
							<input type="hidden" name="on" value="no" />
							<Button type="submit">Release</Button>
						</form>
					{:else}
						<Button onclick={() => (holdOpen = true)}><Pause size={16} /> Hold</Button>
					{/if}
				{/if}
				{#if i.status === 'draft' && data.canApprove && !needsReview}
					<form method="POST" action="?/approve" use:enhance={done()}>
						<input type="hidden" name="version" value={i.version} />
						<Button type="submit" variant="primary" disabled={dirty || i.onHold}>
							<Check size={16} /> Approve
						</Button>
					</form>
				{:else if i.status === 'draft' && data.canApprove && data.review?.canOverride}
					<form method="POST" action="?/approve" use:enhance={done()}>
						<input type="hidden" name="version" value={i.version} />
						<input type="hidden" name="override" value="yes" />
						<Button
							type="submit"
							variant="ghost"
							disabled={dirty || i.onHold}
							title="It needs review; approving it now is recorded"
						>
							Approve without review
						</Button>
					</form>
				{/if}
				{#if i.status === 'approved' && data.canWrite}
					<form method="POST" action="?/sent" use:enhance={done()}>
						<Button type="submit" variant="primary"><Send size={16} /> Mark sent</Button>
					</form>
				{/if}
				{#if i.status === 'sent' && i.kind === 'invoice' && data.canWrite}
					<Button onclick={() => (creditOpen = true)}>Credit memo</Button>
				{/if}
				{#if (i.status === 'draft' || i.status === 'approved') && data.canWrite}
					<form method="POST" action="?/void" use:enhance={done()}>
						<input type="hidden" name="version" value={i.version} />
						<Button type="submit" variant="ghost">Void</Button>
					</form>
				{/if}
			</div>
		</div>
	</div>

	<section class="overflow-x-auto rounded-card border border-border bg-surface" aria-label="Lines">
		<table class="w-full min-w-[720px] text-left text-sm">
			<thead class="border-b border-border text-xs font-bold text-fg-subtle">
				<tr>
					<th scope="col" class="px-3 py-2.5">Description</th>
					<th scope="col" class="w-24 px-3 py-2.5 text-right">Quantity</th>
					<th scope="col" class="w-28 px-3 py-2.5 text-right">Price</th>
					<th scope="col" class="w-28 px-3 py-2.5 text-right">Amount</th>
					<th scope="col" class="w-10 px-3 py-2.5"><span class="sr-only">Remove</span></th>
				</tr>
			</thead>
			<tbody>
				{#each rows as r, idx (r.key)}
					<tr class="border-b border-border last:border-0">
						<td class="px-3 py-1.5">
							<input
								class={cell}
								aria-label="Description"
								bind:value={r.description}
								oninput={() => (dirty = true)}
								disabled={!draft}
							/>
							<span class="px-2 text-[11px] text-fg-subtle">
								{r.kind}{r.edited ? ' · edited by hand' : ''}
							</span>
						</td>
						<td class="px-3 py-1.5">
							<input
								class="{cell} text-right tabular-nums"
								aria-label="Quantity"
								inputmode="decimal"
								bind:value={r.quantity}
								oninput={() => (dirty = true)}
								disabled={!draft}
							/>
						</td>
						<td class="px-3 py-1.5">
							<input
								class="{cell} text-right tabular-nums"
								aria-label="Price"
								inputmode="decimal"
								bind:value={r.unitPrice}
								oninput={() => (dirty = true)}
								disabled={!draft}
							/>
						</td>
						<td class="px-3 py-2.5 text-right font-mono tabular-nums">
							{money(n(r.quantity) * n(r.unitPrice), i.currency)}
						</td>
						<td class="px-3 py-1.5">
							{#if draft}
								<button
									type="button"
									class="grid size-8 place-items-center text-fg-subtle hover:text-danger"
									aria-label="Remove {r.description}"
									onclick={() => (rows.splice(idx, 1), (dirty = true))}
								>
									<Trash2 size={14} />
								</button>
							{/if}
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
						{money(dirty ? total : i.total, i.currency)}
					</td>
					<td></td>
				</tr>
			</tfoot>
		</table>
		{#if draft}
			<div class="border-t border-border px-3 py-2.5">
				<Button
					size="sm"
					onclick={() => (
						rows.push({
							key: ++seq,
							id: null,
							description: '',
							quantity: '1',
							unit: 'each',
							unitPrice: '0',
							edited: true,
							kind: 'manual'
						}),
						(dirty = true)
					)}
				>
					<Plus size={14} /> Line
				</Button>
			</div>
		{/if}
	</section>

	{#if data.review && (i.status === 'draft' || data.review.history.length > 0)}
		<InvoiceReview
			review={data.review}
			approval={data.approval}
			canSubmit={data.canWrite && i.status === 'draft'}
			version={i.version}
			{dirty}
			done={said}
		/>
	{/if}

	<QuoteComments
		comments={data.comments}
		lines={commentLines}
		people={data.people}
		me={data.me}
		done={said}
		whole="The whole invoice"
	/>

	{#if draft || i.notes}
		<Card title="Notes">
			<textarea
				aria-label="Notes"
				rows="3"
				class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				bind:value={notes}
				oninput={() => (dirty = true)}
				disabled={!draft}></textarea>
		</Card>
	{/if}
</div>

<Dialog bind:open={holdOpen} title="Hold the invoice">
	<form
		id="hold-form"
		method="POST"
		action="?/hold"
		class="flex flex-col gap-3"
		use:enhance={done(() => (holdOpen = false))}
	>
		<input type="hidden" name="on" value="yes" />
		<Field label="Why" for="hold-reason">
			<Input id="hold-reason" name="reason" maxlength={500} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (holdOpen = false)}>Cancel</Button>
		<Button type="submit" form="hold-form" variant="primary">Hold</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={creditOpen} title="Credit memo">
	<form
		id="credit-form"
		method="POST"
		action="?/credit"
		class="flex flex-col gap-3"
		use:enhance={done(() => (creditOpen = false))}
	>
		<fieldset class="flex flex-col gap-1">
			<legend class="mb-1 text-sm font-semibold text-fg">Lines to credit (none: all)</legend>
			{#each i.lines as l (l.id)}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="lineIds" value={l.id} />
					{l.description} · {money(l.amount, i.currency)}
				</label>
			{/each}
		</fieldset>
		<Field label="Why" for="credit-reason">
			<Input id="credit-reason" name="reason" maxlength={4000} />
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (creditOpen = false)}>Cancel</Button>
		<Button type="submit" form="credit-form" variant="primary">Make the credit memo</Button>
	{/snippet}
</Dialog>
