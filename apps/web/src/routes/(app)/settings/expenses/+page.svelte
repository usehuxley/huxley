<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { money } from '#lib/expenses/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	type Category = (typeof data.categories)[number];
	let open = $state(false);
	let editing: Category | null = $state(null);

	const said =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
			if (result.type === 'success') after?.();
		};
	const today = () => new Date(Date.now()).toISOString().slice(0, 10);
</script>

<svelte:head>
	<title>Expenses · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Expense categories"
		description="What people log expenses as: whether they're billed (and marked up) and paid back by default, a limit over which they're flagged, and whether a receipt is needed."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					open = true;
				}}
			>
				<Plus size={16} /> New category
			</Button>
		{/snippet}
		<ul class="flex flex-col divide-y divide-border" aria-label="Expense categories">
			{#each data.categories as c (c.id)}
				<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
					<div class="min-w-0">
						<p class="flex flex-wrap items-center gap-2 text-sm font-semibold text-fg">
							{c.name}
							{#if c.kind === 'mileage'}<Badge>Mileage</Badge>{/if}
							{#if !c.active}<Badge>Off</Badge>{/if}
						</p>
						<p class="text-xs text-fg-muted">
							{c.billable
								? `Billed${Number(c.markupPct) > 0 ? ` +${Number(c.markupPct)}%` : ''}`
								: 'Not billed'}
							· {c.reimbursable ? 'paid back' : 'not paid back'}{c.limitAmount
								? ` · flagged over ${money(c.limitAmount)}`
								: ''}{c.receiptRequired && c.kind === 'expense' ? ' · receipt needed' : ''}
						</p>
					</div>
					<Button
						size="sm"
						onclick={() => {
							editing = c;
							open = true;
						}}
					>
						Edit
					</Button>
				</li>
			{/each}
		</ul>
	</Card>

	<Card
		title="Mileage rates"
		description="Mileage is the distance times the rate in effect on the day."
	>
		<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Mileage rates">
			{#each data.rates as r (r.id)}
				<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
					<span class="text-fg">
						{money(r.rate, r.currency)} per {r.unit === 'km' ? 'kilometer' : 'mile'}
						<span class="text-fg-muted">from {r.effectiveFrom}</span>
					</span>
					<form method="POST" action="?/deleteRate" use:enhance={said()}>
						<input type="hidden" name="id" value={r.id} />
						<Button
							type="submit"
							size="sm"
							variant="ghost"
							aria-label="Remove the rate from {r.effectiveFrom}"
						>
							Remove
						</Button>
					</form>
				</li>
			{/each}
		</ul>
		<form
			method="POST"
			action="?/setRate"
			class="mt-3 flex flex-wrap items-end gap-2"
			use:enhance={said()}
		>
			<Field label="Rate" for="rate">
				<Input id="rate" name="rate" inputmode="decimal" required class="w-28" />
			</Field>
			<Field label="Per" for="unit">
				<Select
					id="unit"
					name="unit"
					value="mi"
					options={[
						{ value: 'mi', label: 'Mile' },
						{ value: 'km', label: 'Kilometer' }
					]}
				/>
			</Field>
			<Field label="Currency" for="rate-currency">
				<Input id="rate-currency" name="currency" value="USD" maxlength={3} class="w-20" />
			</Field>
			<Field label="From" for="rate-from">
				<Input id="rate-from" name="effectiveFrom" type="date" value={today()} required />
			</Field>
			<Button type="submit" variant="primary">Set the rate</Button>
		</form>
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New expense category'}>
	<form
		id="category-form"
		method="POST"
		action="?/saveCategory"
		class="grid gap-3 sm:grid-cols-2"
		use:enhance={said(() => (open = false))}
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<div class="sm:col-span-2">
			<Field label="Name" for="category-name">
				<Input
					id="category-name"
					name="name"
					required
					maxlength={100}
					value={editing?.name ?? ''}
				/>
			</Field>
		</div>
		<Field label="Kind" for="category-kind">
			<Select
				id="category-kind"
				name="kind"
				value={editing?.kind ?? 'expense'}
				options={[
					{ value: 'expense', label: 'An amount spent' },
					{ value: 'mileage', label: 'Mileage' }
				]}
			/>
		</Field>
		<Field label="Markup %" for="category-markup">
			<Input
				id="category-markup"
				name="markupPct"
				inputmode="decimal"
				value={editing?.markupPct ?? ''}
			/>
		</Field>
		<Field label="Flag over" for="category-limit">
			<Input
				id="category-limit"
				name="limitAmount"
				inputmode="decimal"
				value={editing?.limitAmount ?? ''}
			/>
		</Field>
		<div class="flex flex-col gap-1 text-sm text-fg sm:col-span-2">
			<label class="flex items-center gap-2">
				<input type="checkbox" name="billable" checked={editing?.billable ?? false} /> Billed to the client
				by default
			</label>
			<label class="flex items-center gap-2">
				<input type="checkbox" name="reimbursable" checked={editing?.reimbursable ?? true} /> Paid back
				by default
			</label>
			<label class="flex items-center gap-2">
				<input type="checkbox" name="receiptRequired" checked={editing?.receiptRequired ?? true} /> Needs
				a receipt
			</label>
			<label class="flex items-center gap-2">
				<input type="checkbox" name="active" checked={editing?.active ?? true} /> In use
			</label>
		</div>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="category-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
