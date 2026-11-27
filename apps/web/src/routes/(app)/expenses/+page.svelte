<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Button, Card, Select } from '@usehuxley/ui';
	import ExpenseDialog from '#lib/expenses/ExpenseDialog.svelte';
	import ExpenseList from '#lib/expenses/ExpenseList.svelte';
	import { EXPENSE_STATUSES, money } from '#lib/expenses/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let open = $state(false);
	let toPayBack = $derived(
		data.entries
			.filter((e) => e.reimbursable && e.userId === data.me && e.status !== 'reimbursed')
			.reduce((t, e) => t + Number(e.amount), 0)
	);
	let currency = $derived(data.entries[0]?.currency ?? 'USD');
</script>

<svelte:head>
	<title>Expenses · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">Expenses</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				{data.everyone ? "Everyone's expenses and mileage." : 'Your expenses and mileage.'}
				{#if !data.everyone && toPayBack > 0}{money(toPayBack, currency)} to be paid back.{/if}
			</p>
		</div>
		{#if data.canWrite}
			<div class="ml-auto">
				<Button variant="primary" onclick={() => (open = true)}>
					<Plus size={16} /> Log an expense
				</Button>
			</div>
		{/if}
	</div>

	<form method="GET" class="flex flex-wrap items-center gap-2">
		{#if data.canReadAll}
			<label for="who" class="sr-only">Whose</label>
			<Select
				id="who"
				name="who"
				value={data.everyone ? 'everyone' : ''}
				options={[
					{ value: '', label: 'Mine' },
					{ value: 'everyone', label: 'Everyone' }
				]}
			/>
		{/if}
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[{ value: '', label: 'Any status' }, ...EXPENSE_STATUSES.map((s) => ({ ...s }))]}
		/>
		<Button type="submit">Show</Button>
		<a href={resolve('/(app)/expenses')} class="text-sm text-fg-muted hover:text-fg">Clear</a>
	</form>

	<Card>
		<ExpenseList entries={data.entries} me={data.me} showWho={data.everyone} showWhere />
	</Card>
</div>

<ExpenseDialog bind:open categories={data.categories} targets={data.targets} />
