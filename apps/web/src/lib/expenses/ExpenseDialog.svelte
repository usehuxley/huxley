<!-- Logging an expense (or mileage): category, day, what, how much (or how
     far), and optionally the receipt. Where it goes comes from the page,
     or the `for` choices (a client or the MSP). -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Button, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Category = components['schemas']['ExpenseCategory'];
	let {
		open = $bindable(false),
		categories,
		action = '?/logExpense',
		targets = [],
		phases = []
	}: {
		open?: boolean;
		categories: Category[];
		action?: string;
		/** When the page doesn't say where it goes: clients and the MSP. */
		targets?: { value: string; label: string }[];
		phases?: { value: string; label: string }[];
	} = $props();

	let categoryId = $state('');
	let category = $derived(categories.find((c) => c.id === categoryId));
	let billable = $state(false);
	let reimbursable = $state(true);
	$effect(() => {
		if (open && !categoryId) categoryId = categories[0]?.id ?? '';
	});
	$effect(() => {
		if (category) {
			billable = category.billable;
			reimbursable = category.reimbursable;
		}
	});
	const today = () => new Date(Date.now()).toISOString().slice(0, 10);

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update();
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			if (result.type === 'success') {
				toast.push({ title: message ?? 'Logged.', tone: 'success' });
				open = false;
			} else if (result.type === 'failure') {
				toast.push({ title: message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<Dialog bind:open title="Log an expense">
	<form
		id="expense-form"
		method="POST"
		{action}
		enctype="multipart/form-data"
		class="grid gap-3 sm:grid-cols-2"
		use:enhance={done}
	>
		<Field label="Category" for="expense-category">
			<Select
				id="expense-category"
				name="categoryId"
				bind:value={categoryId}
				options={categories.map((c) => ({ value: c.id, label: c.name }))}
			/>
		</Field>
		<Field label="Day" for="expense-day">
			<Input id="expense-day" name="spentOn" type="date" required value={today()} />
		</Field>
		<div class="sm:col-span-2">
			<Field label="What for" for="expense-description">
				<Input id="expense-description" name="description" required maxlength={500} />
			</Field>
		</div>
		{#if category?.kind === 'mileage'}
			<Field label="Distance" for="expense-distance">
				<Input id="expense-distance" name="distance" inputmode="decimal" required />
			</Field>
			<Field label="Unit" for="expense-unit">
				<Select
					id="expense-unit"
					name="unit"
					value="mi"
					options={[
						{ value: 'mi', label: 'Miles' },
						{ value: 'km', label: 'Kilometers' }
					]}
				/>
			</Field>
		{:else}
			<Field label="Amount (tax included)" for="expense-amount">
				<Input id="expense-amount" name="amount" inputmode="decimal" required />
			</Field>
			<Field label="Of which tax" for="expense-tax">
				<Input id="expense-tax" name="tax" inputmode="decimal" />
			</Field>
			<Field label="Vendor" for="expense-vendor">
				<Input id="expense-vendor" name="vendor" maxlength={200} />
			</Field>
			<Field label="Receipt" for="expense-receipt">
				<input
					id="expense-receipt"
					name="receipt"
					type="file"
					accept="image/jpeg,image/png,image/gif,image/webp,image/heic,application/pdf"
					class="text-sm text-fg"
				/>
			</Field>
		{/if}
		{#if targets.length}
			<div class="sm:col-span-2">
				<Field label="For" for="expense-org">
					<Select id="expense-org" name="orgId" options={targets} />
				</Field>
			</div>
		{/if}
		{#if phases.length}
			<div class="sm:col-span-2">
				<Field label="Phase" for="expense-phase">
					<Select
						id="expense-phase"
						name="phaseId"
						options={[{ value: '', label: 'The whole project' }, ...phases]}
					/>
				</Field>
			</div>
		{/if}
		<div class="flex flex-wrap gap-4 text-sm text-fg sm:col-span-2">
			<label class="flex items-center gap-2">
				<input type="checkbox" name="billable" bind:checked={billable} /> Bill the client
			</label>
			<label class="flex items-center gap-2">
				<input type="checkbox" name="reimbursable" bind:checked={reimbursable} /> Pay me back
			</label>
		</div>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="expense-form" variant="primary">Log it</Button>
	{/snippet}
</Dialog>
