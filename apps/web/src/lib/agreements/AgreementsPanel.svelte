<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus, Trash2 } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		Dialog,
		EmptyState,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { hours, KINDS, kindLabel, OUTCOME, type AgreementKind } from './coverage.ts';

	type Agreement = components['schemas']['Agreement'];
	type Usage = components['schemas']['AgreementUsage'];
	type WorkType = components['schemas']['WorkType'];
	type AssetType = components['schemas']['AssetType'];
	type Product = components['schemas']['CatalogProduct'];
	type Status = components['schemas']['AgreementStatus'];
	type LineBilling = components['schemas']['AgreementLineBilling'];

	type Props = {
		agreements: Agreement[];
		usage: Record<string, Usage | null>;
		workTypes: WorkType[];
		assetTypes: AssetType[];
		products: Product[];
		canManage: boolean;
		errors: Record<string, string>;
	};
	let { agreements, usage, workTypes, assetTypes, products, canManage, errors }: Props = $props();

	const STATUS: { value: Status; label: string }[] = [
		{ value: 'draft', label: 'Draft' },
		{ value: 'active', label: 'Active' },
		{ value: 'cancelled', label: 'Cancelled' }
	];
	const BILLING: { value: LineBilling; label: string }[] = [
		{ value: 'monthly', label: 'Monthly' },
		{ value: 'quarterly', label: 'Quarterly' },
		{ value: 'annual', label: 'Annual' }
	];
	let workTypeName = $derived(new Map(workTypes.map((w) => [w.id, w.name])));
	let assetTypeName = $derived(new Map(assetTypes.map((t) => [t.id, t.name])));
	const covers = (k: AgreementKind) => k === 'managed' || k === 'block_hours';

	// The agreement being added or changed.
	let open = $state(false);
	let editing = $state<Agreement | null>(null);
	let name = $state('');
	let kind = $state<AgreementKind>('managed');
	let status = $state<Status>('draft');
	let startsOn = $state('');
	let endsOn = $state('');
	let currency = $state('USD');
	let priority = $state('0');
	let increment = $state('1');
	let coversAll = $state(true);
	let chosenWorkTypes: string[] = $state([]);
	let chosenAssetTypes: string[] = $state([]);
	let allowanceHours = $state('');
	let autoRenew = $state(false);
	let noticeDays = $state('30');
	let notes = $state('');
	let defaultRate = $state('');
	let rates: { workTypeId: string; hourlyRate: string }[] = $state([]);
	let lines: {
		productId: string;
		description: string;
		quantity: string;
		unitPrice: string;
		billing: LineBilling;
	}[] = $state([]);

	function edit(a: Agreement | null) {
		editing = a;
		name = a?.name ?? '';
		kind = a?.kind ?? 'managed';
		status = a?.status ?? 'draft';
		startsOn = a?.startsOn ?? new Date().toISOString().slice(0, 10);
		endsOn = a?.endsOn ?? '';
		currency = a?.currency ?? 'USD';
		priority = String(a?.priority ?? 0);
		increment = String(a?.billingIncrement ?? 1);
		coversAll = a?.coversAllWorkTypes ?? true;
		chosenWorkTypes = [...(a?.workTypeIds ?? [])];
		chosenAssetTypes = [...(a?.assetTypeIds ?? [])];
		const allowance = a?.kind === 'block_hours' ? a.blockMinutes : a?.monthlyMinutes;
		allowanceHours = allowance ? String(allowance / 60) : '';
		autoRenew = a?.autoRenew ?? false;
		noticeDays = String(a?.renewalNoticeDays ?? 30);
		notes = a?.notes ?? '';
		defaultRate = a?.rates.find((r) => !r.workTypeId)?.hourlyRate ?? '';
		rates = (a?.rates ?? [])
			.filter((r) => r.workTypeId)
			.map((r) => ({ workTypeId: r.workTypeId ?? '', hourlyRate: r.hourlyRate }));
		lines = (a?.lines ?? []).map((l) => ({
			productId: l.productId ?? '',
			description: l.description,
			quantity: l.quantity,
			unitPrice: l.unitPrice ?? '',
			billing: l.billing
		}));
		open = true;
	}

	let allowanceMinutes = $derived(
		allowanceHours.trim() ? Math.round(Number(allowanceHours) * 60) : null
	);
	let agreementJson = $derived(
		JSON.stringify({
			name,
			kind,
			status,
			startsOn,
			endsOn: endsOn || null,
			currency,
			priority: Number(priority) || 0,
			billingIncrement: Number(increment) || 1,
			coversAllWorkTypes: covers(kind) ? coversAll : true,
			workTypeIds: covers(kind) && !coversAll ? chosenWorkTypes : [],
			assetTypeIds: covers(kind) ? chosenAssetTypes : [],
			monthlyMinutes: kind === 'managed' ? allowanceMinutes : null,
			blockMinutes: kind === 'block_hours' ? allowanceMinutes : null,
			autoRenew,
			renewalNoticeDays: Number(noticeDays) || 0,
			notes,
			rates: [
				...(defaultRate.trim() ? [{ hourlyRate: defaultRate }] : []),
				...rates.filter((r) => r.workTypeId && r.hourlyRate.trim())
			],
			lines: lines
				.filter((l) => l.productId || l.description.trim())
				.map((l) => ({
					productId: l.productId || null,
					description: l.description.trim() || null,
					quantity: l.quantity,
					unitPrice: l.unitPrice.trim() || null,
					billing: l.billing
				})),
			...(editing ? { version: editing.version } : {})
		})
	);

	function toggle(list: string[], id: string, on: boolean): string[] {
		return on ? [...list, id] : list.filter((x) => x !== id);
	}

	function coverage(a: Agreement): string {
		if (!covers(a.kind)) return 'All billable work at the agreed rates';
		const what = a.coversAllWorkTypes
			? 'All work'
			: a.workTypeIds.map((w) => workTypeName.get(w) ?? 'Unknown').join(', ');
		const assets =
			a.assetTypeIds.length > 0
				? ` on ${a.assetTypeIds.map((t) => assetTypeName.get(t) ?? 'Unknown').join(', ')}`
				: '';
		const allowance =
			a.kind === 'block_hours' && a.blockMinutes
				? `, a block of ${hours(a.blockMinutes)}`
				: a.monthlyMinutes
					? `, up to ${hours(a.monthlyMinutes)} a month`
					: '';
		return `${what}${assets}${allowance}`;
	}

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const saved =
					(result.data as { agreement?: { saved?: string } } | undefined)?.agreement?.saved ??
					'Saved.';
				toast.push({ title: saved, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failure = result.data as { agreement?: { message?: string } } | undefined;
				toast.push({
					title: failure?.agreement?.message ?? 'Something went wrong.',
					tone: 'danger'
				});
			}
		};
</script>

<Card
	title="Agreements"
	description="What this client has agreed to, what it covers, and how this month's time falls under it."
>
	{#snippet actions()}
		{#if canManage}
			<Button size="sm" onclick={() => edit(null)}><Plus size={14} /> New agreement</Button>
		{/if}
	{/snippet}
	{#if agreements.length === 0}
		<EmptyState title="No agreements" description="Time on this client's tickets is billable." />
	{:else}
		<ul class="flex flex-col divide-y divide-border" aria-label="Agreements">
			{#each agreements as a (a.id)}
				{@const used = usage[a.id]}
				<li class="flex flex-col gap-2 py-3 first:pt-0">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<div class="flex flex-wrap items-center gap-2">
							{#if canManage}
								<button
									type="button"
									class="font-medium text-fg hover:underline"
									onclick={() => edit(a)}
								>
									{a.name}
								</button>
							{:else}
								<span class="font-medium text-fg">{a.name}</span>
							{/if}
							<Badge>{kindLabel(a.kind)}</Badge>
							<Badge
								tone={a.status === 'active'
									? 'success'
									: a.status === 'cancelled'
										? 'neutral'
										: 'warning'}
							>
								{STATUS.find((s) => s.value === a.status)?.label}
							</Badge>
						</div>
						<span class="text-xs text-fg-muted">
							{a.startsOn} – {a.endsOn ?? 'ongoing'}{a.autoRenew ? ' · renews' : ''}
						</span>
					</div>
					<p class="text-sm text-fg-muted">{coverage(a)}</p>
					{#if a.rates.length > 0}
						<p class="text-xs text-fg-muted">
							Rates: {a.rates
								.map((r) => `${r.workTypeName ?? 'Default'} ${r.hourlyRate} ${a.currency}/h`)
								.join(' · ')}
						</p>
					{/if}
					{#if a.lines.length > 0}
						<ul class="text-xs text-fg-muted" aria-label="Lines of {a.name}">
							{#each a.lines as l (l.id)}
								<li>
									{l.quantity} × {l.description}{l.effectivePrice
										? ` at ${l.effectivePrice} ${a.currency}`
										: ''}, {l.billing}
								</li>
							{/each}
						</ul>
					{/if}
					{#if used}
						<div class="text-sm" data-testid="agreement-usage">
							<p class="text-fg">
								This month: {hours(used.coveredMinutes)} covered, {hours(used.billableMinutes)} billable{used.amount
									? ` (${used.amount} ${used.currency})`
									: ''}
							</p>
							{#if used.entries.length > 0}
								<ul class="mt-1 text-xs text-fg-muted">
									{#each used.entries as e (e.timeEntryId)}
										<li class="flex flex-wrap items-center gap-1">
											<Badge tone={OUTCOME[e.outcome].tone}>{OUTCOME[e.outcome].label}</Badge>
											{e.workedOn} · {e.ticketReference ?? ''} · {e.workTypeName} · {hours(
												e.minutes
											)}
											{#if e.billableMinutes > 0}· {hours(e.billableMinutes)} billable{/if}
										</li>
									{/each}
								</ul>
							{/if}
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</Card>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New agreement'}>
	<form
		method="POST"
		action="?tab=agreements&/agreementSave"
		class="flex flex-col gap-4"
		use:enhance={done(() => (open = false))}
	>
		{#if editing}<input type="hidden" name="agreementId" value={editing.id} />{/if}
		<input type="hidden" name="agreement" value={agreementJson} />
		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Agreement name" for="ag-name" error={errors.name}>
				<Input id="ag-name" bind:value={name} required maxlength={200} />
			</Field>
			<Field label="Kind" for="ag-kind" error={errors.kind}>
				<Select id="ag-kind" bind:value={kind} options={KINDS} />
			</Field>
			<Field label="Status" for="ag-status" error={errors.status}>
				<Select id="ag-status" bind:value={status} options={STATUS} />
			</Field>
			<Field label="Currency" for="ag-currency" error={errors.currency}>
				<Input id="ag-currency" bind:value={currency} maxlength={3} />
			</Field>
			<Field label="Starts" for="ag-starts" error={errors.startsOn}>
				<Input id="ag-starts" type="date" bind:value={startsOn} required />
			</Field>
			<Field label="Ends" for="ag-ends" hint="Leave empty if it's ongoing" error={errors.endsOn}>
				<Input id="ag-ends" type="date" bind:value={endsOn} />
			</Field>
		</div>

		{#if covers(kind)}
			<fieldset class="flex flex-col gap-2">
				<legend class="mb-1 text-sm font-medium text-fg">What it covers</legend>
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" bind:checked={coversAll} /> All work types
				</label>
				{#if !coversAll}
					<div class="flex flex-wrap gap-x-4 gap-y-1" role="group" aria-label="Covered work types">
						{#each workTypes as w (w.id)}
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									checked={chosenWorkTypes.includes(w.id)}
									onchange={(e) =>
										(chosenWorkTypes = toggle(chosenWorkTypes, w.id, e.currentTarget.checked))}
								/>
								{w.name}
							</label>
						{/each}
					</div>
					{#if errors.workTypeIds}<p class="text-sm text-danger">{errors.workTypeIds}</p>{/if}
				{/if}
				{#if assetTypes.length > 0}
					<p class="mt-1 text-sm text-fg-muted">
						Only tickets about these kinds of asset (none ticked: any ticket):
					</p>
					<div class="flex flex-wrap gap-x-4 gap-y-1" role="group" aria-label="Covered asset types">
						{#each assetTypes as t (t.id)}
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									checked={chosenAssetTypes.includes(t.id)}
									onchange={(e) =>
										(chosenAssetTypes = toggle(chosenAssetTypes, t.id, e.currentTarget.checked))}
								/>
								{t.name}
							</label>
						{/each}
					</div>
				{/if}
				<div class="w-56">
					<Field
						label={kind === 'block_hours' ? 'Hours in the block' : 'Hours included a month'}
						for="ag-allowance"
						hint={kind === 'managed' ? 'Leave empty for unlimited' : undefined}
						error={errors.blockMinutes ?? errors.monthlyMinutes}
					>
						<Input id="ag-allowance" bind:value={allowanceHours} inputmode="decimal" />
					</Field>
				</div>
			</fieldset>
		{/if}

		<fieldset class="flex flex-col gap-2">
			<legend class="mb-1 text-sm font-medium text-fg">Rates for billable time (per hour)</legend>
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="Default rate" for="ag-rate" error={errors['rates/0/hourlyRate']}>
					<Input id="ag-rate" bind:value={defaultRate} inputmode="decimal" placeholder="0.00" />
				</Field>
				<Field
					label="Billing increment (minutes)"
					for="ag-increment"
					error={errors.billingIncrement}
				>
					<Input id="ag-increment" bind:value={increment} inputmode="numeric" />
				</Field>
			</div>
			{#each rates as r, i (i)}
				<div class="flex items-end gap-2" role="group" aria-label="Rate {i + 1}">
					<div class="min-w-0 flex-1">
						<Field label="Work type" for="rate-wt-{i}">
							<Select
								id="rate-wt-{i}"
								bind:value={r.workTypeId}
								options={[
									{ value: '', label: 'Choose' },
									...workTypes.map((w) => ({ value: w.id, label: w.name }))
								]}
							/>
						</Field>
					</div>
					<div class="w-28">
						<Field label="Rate" for="rate-{i}">
							<Input id="rate-{i}" bind:value={r.hourlyRate} inputmode="decimal" />
						</Field>
					</div>
					<button
						type="button"
						class="pb-2 text-fg-subtle hover:text-danger"
						aria-label="Remove rate {i + 1}"
						onclick={() => (rates = rates.filter((_, j) => j !== i))}
					>
						<Trash2 size={16} />
					</button>
				</div>
			{/each}
			<div>
				<Button
					size="sm"
					variant="secondary"
					onclick={() => (rates = [...rates, { workTypeId: '', hourlyRate: '' }])}
				>
					<Plus size={14} /> A rate for a work type
				</Button>
			</div>
		</fieldset>

		<fieldset class="flex flex-col gap-2">
			<legend class="mb-1 text-sm font-medium text-fg">Recurring lines (the fee)</legend>
			{#each lines as l, i (i)}
				<div
					class="grid gap-2 rounded-md border border-border p-2 sm:grid-cols-2"
					role="group"
					aria-label="Line {i + 1}"
				>
					{#if products.length > 0}
						<Field label="Product" for="line-product-{i}" error={errors[`lines/${i}/productId`]}>
							<Select
								id="line-product-{i}"
								bind:value={l.productId}
								options={[
									{ value: '', label: 'None (describe it)' },
									...products.map((p) => ({ value: p.id, label: `${p.name} (${p.sku})` }))
								]}
							/>
						</Field>
					{/if}
					<Field label="Description" for="line-desc-{i}" error={errors[`lines/${i}/description`]}>
						<Input id="line-desc-{i}" bind:value={l.description} maxlength={500} />
					</Field>
					<Field label="Quantity" for="line-qty-{i}" error={errors[`lines/${i}/quantity`]}>
						<Input id="line-qty-{i}" bind:value={l.quantity} inputmode="decimal" />
					</Field>
					<Field
						label="Unit price"
						for="line-price-{i}"
						hint="Empty: the product's price"
						error={errors[`lines/${i}/unitPrice`]}
					>
						<Input id="line-price-{i}" bind:value={l.unitPrice} inputmode="decimal" />
					</Field>
					<Field label="Billed" for="line-billing-{i}">
						<Select id="line-billing-{i}" bind:value={l.billing} options={BILLING} />
					</Field>
					<div class="flex items-end">
						<Button
							size="sm"
							variant="ghost"
							onclick={() => (lines = lines.filter((_, j) => j !== i))}
						>
							<Trash2 size={14} /> Remove line
						</Button>
					</div>
				</div>
			{/each}
			<div>
				<Button
					size="sm"
					variant="secondary"
					onclick={() =>
						(lines = [
							...lines,
							{ productId: '', description: '', quantity: '1', unitPrice: '', billing: 'monthly' }
						])}
				>
					<Plus size={14} /> Add a line
				</Button>
			</div>
		</fieldset>

		<div class="grid gap-3 sm:grid-cols-3">
			<Field label="Priority" for="ag-priority" hint="Lower goes first" error={errors.priority}>
				<Input id="ag-priority" bind:value={priority} inputmode="numeric" />
			</Field>
			<Field label="Renewal notice (days)" for="ag-notice" error={errors.renewalNoticeDays}>
				<Input id="ag-notice" bind:value={noticeDays} inputmode="numeric" />
			</Field>
			<label class="flex items-end gap-2 pb-2 text-sm text-fg">
				<input type="checkbox" bind:checked={autoRenew} /> Renews automatically
			</label>
		</div>
		<Field label="Notes" for="ag-notes" error={errors.notes}>
			<textarea
				id="ag-notes"
				bind:value={notes}
				rows="2"
				maxlength="4000"
				class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
			></textarea>
		</Field>
		<div class="flex justify-between gap-2">
			<div>
				{#if editing && editing.status === 'draft'}
					<Button type="submit" variant="danger" formaction="?tab=agreements&/agreementDelete">
						Delete draft
					</Button>
				{/if}
			</div>
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</div>
	</form>
</Dialog>
