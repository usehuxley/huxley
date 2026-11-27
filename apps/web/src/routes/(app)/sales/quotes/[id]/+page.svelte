<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import {
		ArrowDown,
		ArrowUp,
		Check,
		Download,
		Heading,
		Plus,
		Send,
		Trash2,
		X
	} from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast, tray } from '@usehuxley/ui';
	import {
		BILLINGS,
		QUOTE_STATUSES,
		label,
		money,
		quoteRef,
		quoteTone
	} from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let q = $derived(data.quote);
	let editable = $derived(data.canWrite && (q.status === 'draft' || q.status === 'sent'));

	type Billing = 'one_time' | 'monthly' | 'quarterly' | 'annual';
	type Row = {
		key: number;
		kind: 'item' | 'heading';
		productId: string | null;
		sku: string;
		name: string;
		description: string;
		billing: Billing;
		quantity: string;
		unit: string;
		unitCost: string;
		unitPrice: string;
		discountPct: string;
		optional: boolean;
		selected: boolean;
	};

	let seq = 0;
	let title = $state('');
	let validUntil = $state('');
	let contactId = $state('');
	let intro = $state('');
	let terms = $state('');
	let rows = $state<Row[]>([]);
	let dirty = $state(false);

	// Start from the saved quote, and again whenever it's saved or reloaded.
	$effect(() => {
		const saved = data.quote;
		title = saved.title;
		validUntil = saved.validUntil ?? '';
		contactId = saved.contactId ?? '';
		intro = saved.intro;
		terms = saved.terms;
		rows = saved.lines.map((l) => ({
			key: ++seq,
			kind: l.kind,
			productId: l.productId ?? null,
			sku: l.sku ?? '',
			name: l.name,
			description: l.description,
			billing: l.billing,
			quantity: l.quantity,
			unit: l.unit,
			unitCost: l.unitCost,
			unitPrice: l.unitPrice,
			discountPct: l.discountPct,
			optional: l.optional,
			selected: l.selected
		}));
		dirty = false;
	});

	$effect(() => {
		tray.open({
			key: `quote:${q.id}`,
			kind: 'other',
			href: `/sales/quotes/${q.id}`,
			title: q.title,
			subtitle: `${quoteRef(q.number)} · ${q.orgName}`
		});
	});

	const n = (s: string) => {
		const v = Number(s);
		return Number.isFinite(v) ? v : 0;
	};
	const lineTotal = (r: Row) =>
		Math.round(n(r.quantity) * n(r.unitPrice) * (100 - n(r.discountPct))) / 100;
	const perMonth = (b: Billing, amount: number) =>
		b === 'monthly' ? amount : b === 'quarterly' ? amount / 3 : b === 'annual' ? amount / 12 : null;

	// The same totals the API works out, live while editing.
	let totals = $derived.by(() => {
		let oneTime = 0,
			monthly = 0,
			cost = 0;
		for (const r of rows) {
			if (r.kind !== 'item' || (r.optional && !r.selected)) continue;
			const total = lineTotal(r);
			const c = n(r.quantity) * n(r.unitCost);
			const m = perMonth(r.billing, total);
			if (m === null) {
				oneTime += total;
				cost += c;
			} else {
				monthly += m;
				cost += 12 * (perMonth(r.billing, c) ?? 0);
			}
		}
		const firstYear = oneTime + 12 * monthly;
		return {
			oneTime,
			monthly,
			firstYear,
			margin: firstYear > 0 ? Math.round(((firstYear - cost) / firstYear) * 1000) / 10 : null
		};
	});

	function change() {
		dirty = true;
	}
	function add(kind: 'item' | 'heading') {
		rows.push({
			key: ++seq,
			kind,
			productId: null,
			sku: '',
			name: kind === 'heading' ? 'New section' : '',
			description: '',
			billing: 'one_time',
			quantity: '1',
			unit: 'each',
			unitCost: '0',
			unitPrice: '0',
			discountPct: '0',
			optional: false,
			selected: true
		});
		change();
	}
	function addProduct(id: string) {
		const p = data.products.find((x) => x.id === id);
		if (!p) return;
		rows.push({
			key: ++seq,
			kind: 'item',
			productId: p.id,
			sku: p.sku,
			name: p.name,
			description: '',
			billing: p.billing,
			quantity: '1',
			unit: p.unit,
			unitCost: p.cost ?? '0',
			unitPrice: p.price ?? '0',
			discountPct: '0',
			optional: false,
			selected: true
		});
		change();
	}
	function move(i: number, by: number) {
		const j = i + by;
		if (j < 0 || j >= rows.length) return;
		[rows[i], rows[j]] = [rows[j], rows[i]];
		change();
	}

	let doc = $derived(
		JSON.stringify({
			title,
			opportunityId: q.opportunityId ?? null,
			currency: q.currency,
			validUntil: validUntil || null,
			contactId: contactId || null,
			ownerId: q.ownerId ?? null,
			intro,
			terms,
			version: q.version,
			lines: rows.map((r) => ({
				kind: r.kind,
				productId: r.productId,
				sku: r.sku || null,
				name: r.name,
				description: r.description,
				billing: r.billing,
				quantity: r.quantity || '0',
				unit: r.unit || 'each',
				unitCost: r.unitCost || '0',
				unitPrice: r.unitPrice || '0',
				discountPct: r.discountPct || '0',
				optional: r.optional,
				selected: r.selected
			}))
		})
	);

	let deciding = $state<'yes' | 'no' | null>(null);
	let decideOpen = $state(false);
	let options = $derived(q.lines.filter((l) => l.optional));

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
	const cell =
		'h-8 w-full rounded-control border border-border bg-surface px-2 text-sm text-fg disabled:border-transparent disabled:bg-transparent';
</script>

<svelte:head>
	<title>{q.title} · Quotes · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-7xl flex-col gap-5">
	<div>
		<a href={resolve('/(app)/sales/quotes')} class="text-sm text-fg-muted hover:text-fg">
			← Quotes
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div class="min-w-0">
				<p class="font-mono text-[12.5px] text-fg-subtle">
					{quoteRef(q.number)} · revision {q.revision} ·
					<a
						href={resolve('/(app)/clients/[id]', { id: q.orgId })}
						class="hover:text-fg hover:underline"
					>
						{q.orgName}
					</a>
					{#if q.opportunityId}
						· <a
							href={resolve('/(app)/sales/opportunities/[id]', { id: q.opportunityId })}
							class="hover:text-fg hover:underline"
						>
							{q.opportunityName}
						</a>
					{/if}
				</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-balance text-fg">{q.title}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					<Badge tone={quoteTone(q.status)}>{label(QUOTE_STATUSES, q.status)}</Badge>
					{#if q.decidedBy}<Badge>
							{q.status === 'accepted' ? 'Accepted' : 'Declined'} by {q.decidedBy}
						</Badge>{/if}
					{#if dirty}<Badge tone="warning">Unsaved changes</Badge>{/if}
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button href="{resolve('/(app)/sales/quotes/[id]', { id: q.id })}/pdf" variant="secondary">
					<Download size={16} /> PDF
				</Button>
				{#if editable}
					<form
						method="POST"
						action="?/save"
						use:enhance={done(
							q.status === 'sent' ? `Saved as revision ${q.revision + 1}.` : 'Saved.'
						)}
					>
						<input type="hidden" name="doc" value={doc} />
						<Button type="submit" variant={dirty ? 'primary' : 'secondary'}>Save</Button>
					</form>
				{/if}
				{#if data.canWrite && q.status === 'draft'}
					<form method="POST" action="?/send" use:enhance={done('Marked sent.')}>
						<input type="hidden" name="version" value={q.version} />
						<Button
							type="submit"
							variant="primary"
							disabled={dirty}
							title={dirty ? 'Save first' : undefined}
						>
							<Send size={16} /> Mark sent
						</Button>
					</form>
				{/if}
				{#if data.canWrite && q.status === 'sent'}
					<Button variant="primary" onclick={() => ((deciding = 'yes'), (decideOpen = true))}>
						<Check size={16} /> Accepted
					</Button>
					<Button onclick={() => ((deciding = 'no'), (decideOpen = true))}>
						<X size={16} /> Declined
					</Button>
				{/if}
			</div>
		</div>
		{#if q.status === 'sent' && editable}
			<p class="mt-3 text-sm text-fg-subtle">
				Saving changes starts revision {q.revision + 1}; send it again when it's ready.
			</p>
		{/if}
		{#if q.declineReason}
			<p class="mt-3 rounded-control bg-danger-soft px-3 py-2 text-sm text-fg">
				Declined: {q.declineReason}
			</p>
		{/if}
	</div>

	<div class="grid gap-5 2xl:grid-cols-[minmax(0,1fr)_18rem]">
		<div class="flex min-w-0 flex-col gap-5">
			<Card>
				<div class="grid gap-4 sm:grid-cols-[minmax(0,1fr)_10rem_14rem]">
					<Field label="Title" for="title">
						<Input
							id="title"
							bind:value={title}
							oninput={change}
							disabled={!editable}
							maxlength={200}
						/>
					</Field>
					<Field label="Valid until" for="validUntil">
						<Input
							id="validUntil"
							type="date"
							bind:value={validUntil}
							oninput={change}
							disabled={!editable}
						/>
					</Field>
					<Field label="For" for="contactId">
						<Select
							id="contactId"
							bind:value={contactId}
							onchange={change}
							disabled={!editable}
							options={[{ value: '', label: 'No contact' }, ...data.contacts]}
						/>
					</Field>
				</div>
			</Card>

			<section
				class="overflow-x-auto rounded-card border border-border bg-surface"
				aria-label="Lines"
			>
				<table class="w-full min-w-[880px] text-left text-sm">
					<thead class="border-b border-border text-xs font-bold text-fg-subtle">
						<tr>
							<th scope="col" class="w-14 px-2 py-2.5"><span class="sr-only">Order</span></th>
							<th scope="col" class="px-2 py-2.5">Item</th>
							<th scope="col" class="w-28 px-2 py-2.5">Billing</th>
							<th scope="col" class="w-16 px-2 py-2.5">Qty</th>
							<th scope="col" class="w-24 px-2 py-2.5">Unit price</th>
							<th scope="col" class="w-16 px-2 py-2.5">Disc %</th>
							<th scope="col" class="w-24 px-2 py-2.5">Unit cost</th>
							<th scope="col" class="w-28 px-2 py-2.5 text-right">Total</th>
							<th scope="col" class="w-16 px-2 py-2.5 text-center">Option</th>
							<th scope="col" class="w-10 px-2 py-2.5"><span class="sr-only">Remove</span></th>
						</tr>
					</thead>
					<tbody>
						{#each rows as r, i (r.key)}
							<tr
								class="border-b border-border align-top last:border-0 {r.kind === 'heading'
									? 'bg-surface-sunken'
									: ''}"
							>
								<td class="px-2 py-1.5">
									{#if editable}
										<div class="flex">
											<button
												type="button"
												class="grid size-6 place-items-center rounded text-fg-subtle hover:bg-surface-sunken disabled:opacity-30"
												disabled={i === 0}
												onclick={() => move(i, -1)}
												aria-label="Move up"
											>
												<ArrowUp size={13} />
											</button>
											<button
												type="button"
												class="grid size-6 place-items-center rounded text-fg-subtle hover:bg-surface-sunken disabled:opacity-30"
												disabled={i === rows.length - 1}
												onclick={() => move(i, 1)}
												aria-label="Move down"
											>
												<ArrowDown size={13} />
											</button>
										</div>
									{/if}
								</td>
								{#if r.kind === 'heading'}
									<td class="px-2 py-1.5" colspan="8">
										<input
											class="{cell} font-bold"
											aria-label="Section heading"
											bind:value={r.name}
											oninput={change}
											disabled={!editable}
										/>
									</td>
								{:else}
									<td class="px-2 py-1.5">
										<input
											class={cell}
											aria-label="Item name"
											bind:value={r.name}
											oninput={change}
											disabled={!editable}
											placeholder="What it is"
										/>
										{#if r.sku}<span
												class="mt-0.5 block px-2 font-mono text-[11.5px] text-fg-subtle"
											>
												{r.sku}
											</span>{/if}
									</td>
									<td class="px-2 py-1.5">
										<select
											class={cell}
											aria-label="Billing"
											bind:value={r.billing}
											onchange={change}
											disabled={!editable}
										>
											{#each BILLINGS as b (b.value)}<option value={b.value}>
													{b.label}
												</option>{/each}
										</select>
									</td>
									<td class="px-2 py-1.5">
										<input
											class="{cell} text-right tabular-nums"
											aria-label="Quantity"
											inputmode="decimal"
											bind:value={r.quantity}
											oninput={change}
											disabled={!editable}
										/>
									</td>
									<td class="px-2 py-1.5">
										<input
											class="{cell} text-right tabular-nums"
											aria-label="Unit price"
											inputmode="decimal"
											bind:value={r.unitPrice}
											oninput={change}
											disabled={!editable}
										/>
									</td>
									<td class="px-2 py-1.5">
										<input
											class="{cell} text-right tabular-nums"
											aria-label="Discount percent"
											inputmode="decimal"
											bind:value={r.discountPct}
											oninput={change}
											disabled={!editable}
										/>
									</td>
									<td class="px-2 py-1.5">
										<input
											class="{cell} text-right text-fg-muted tabular-nums"
											aria-label="Unit cost"
											inputmode="decimal"
											bind:value={r.unitCost}
											oninput={change}
											disabled={!editable}
										/>
									</td>
									<td
										class="px-2 py-2.5 text-right font-mono font-semibold tabular-nums {r.optional &&
										!r.selected
											? 'text-fg-subtle line-through'
											: 'text-fg'}"
									>
										{money(lineTotal(r), q.currency)}{#if r.billing !== 'one_time'}<span
												class="block text-[11px] font-normal text-fg-subtle"
											>
												{label(BILLINGS, r.billing).toLowerCase()}
											</span>{/if}
									</td>
									<td class="px-2 py-2.5 text-center">
										<input
											type="checkbox"
											aria-label="An option the client may take"
											bind:checked={r.optional}
											onchange={change}
											disabled={!editable}
										/>
									</td>
								{/if}
								<td class="px-2 py-1.5">
									{#if editable}
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
								<td colspan="10" class="px-4 py-8 text-center text-sm text-fg-subtle">
									Nothing quoted yet. Add from the catalog, or write a line.
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				{#if editable}
					<div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2.5">
						<label for="add-product" class="sr-only">Add from the catalog</label>
						<select
							id="add-product"
							class="h-8 max-w-xs rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
							value=""
							onchange={(e) => {
								addProduct(e.currentTarget.value);
								e.currentTarget.value = '';
							}}
						>
							<option value="">Add from the catalog…</option>
							{#each data.products as p (p.id)}
								<option value={p.id}>
									{p.name} · {p.sku}{p.price ? ` · ${money(p.price, q.currency)}` : ''}
								</option>
							{/each}
						</select>
						<Button size="sm" onclick={() => add('item')}><Plus size={14} /> Line</Button>
						<Button size="sm" variant="ghost" onclick={() => add('heading')}>
							<Heading size={14} /> Section
						</Button>
					</div>
				{/if}
			</section>

			<Card title="Wording">
				<div class="grid gap-4 md:grid-cols-2">
					<Field label="Introduction" for="intro">
						<textarea
							id="intro"
							rows="5"
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
							bind:value={intro}
							oninput={change}
							disabled={!editable}></textarea>
					</Field>
					<Field label="Terms" for="terms">
						<textarea
							id="terms"
							rows="5"
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
							bind:value={terms}
							oninput={change}
							disabled={!editable}></textarea>
					</Field>
				</div>
			</Card>
		</div>

		<aside class="grid gap-5 sm:grid-cols-2 2xl:flex 2xl:flex-col">
			<Card title="Totals">
				<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm" aria-label="Totals">
					<dt class="text-fg-muted">One-time</dt>
					<dd class="text-right font-mono tabular-nums">{money(totals.oneTime, q.currency)}</dd>
					<dt class="text-fg-muted">Monthly</dt>
					<dd class="text-right font-mono tabular-nums">{money(totals.monthly, q.currency)}</dd>
					<dt class="font-semibold text-fg">First year</dt>
					<dd class="text-right font-mono font-semibold tabular-nums">
						{money(totals.firstYear, q.currency)}
					</dd>
					<dt class="text-fg-muted">Margin</dt>
					<dd class="text-right font-mono tabular-nums">
						{totals.margin != null ? `${totals.margin}%` : '—'}
					</dd>
				</dl>
				{#if options.length > 0}
					<p class="mt-3 text-xs text-fg-subtle">Options count once the client takes them.</p>
				{/if}
			</Card>
			<Card title="Revisions">
				{#if data.revisions.length === 0}
					<p class="text-sm text-fg-muted">Not sent yet. Each send keeps what was sent.</p>
				{:else}
					<ol class="flex flex-col gap-2 text-sm" aria-label="Revisions">
						{#each data.revisions as r (r.revision)}
							<li>
								<span class="font-semibold text-fg">Revision {r.revision}</span>
								<span class="block text-xs text-fg-subtle">
									Sent {new Date(r.sentAt).toLocaleString()}{r.sentByName
										? ` by ${r.sentByName}`
										: ''}
								</span>
							</li>
						{/each}
					</ol>
				{/if}
			</Card>
			{#if data.canWrite && data.revisions.length === 0}
				<form method="POST" action="?/delete" use:enhance>
					<Button type="submit" variant="ghost">Delete this draft</Button>
				</form>
			{/if}
		</aside>
	</div>
</div>

<Dialog
	bind:open={decideOpen}
	title={deciding === 'yes' ? 'Record the acceptance' : 'Record the decline'}
>
	<form
		id="decide-form"
		method="POST"
		action="?/decide"
		class="flex flex-col gap-4"
		use:enhance={done(deciding === 'yes' ? 'Accepted.' : 'Declined.', () => (decideOpen = false))}
	>
		<input type="hidden" name="version" value={q.version} />
		<input type="hidden" name="accept" value={deciding ?? ''} />
		<Field label={deciding === 'yes' ? 'Accepted by' : 'Declined by'} for="decided-name">
			<Input id="decided-name" name="name" required maxlength={200} value={q.contactName ?? ''} />
		</Field>
		{#if deciding === 'yes' && options.length > 0}
			<fieldset class="flex flex-col gap-2">
				<legend class="mb-1 text-sm font-semibold text-fg">Options taken</legend>
				{#each options as o (o.id)}
					<label class="flex items-center gap-2 text-sm text-fg">
						<input type="checkbox" name="options" value={o.id} />
						{o.name} · {money(o.total, q.currency)}
					</label>
				{/each}
			</fieldset>
		{/if}
		{#if deciding === 'no'}
			<Field label="Why" for="decline-reason">
				<Input id="decline-reason" name="reason" maxlength={2000} />
			</Field>
		{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (decideOpen = false)}>Cancel</Button>
		<Button type="submit" form="decide-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
