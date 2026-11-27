<!-- Expenses on a ticket or project, beside its time. -->
<script lang="ts">
	import { Plus } from '@lucide/svelte';
	import { Button, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import ExpenseDialog from '#lib/expenses/ExpenseDialog.svelte';
	import ExpenseList from '#lib/expenses/ExpenseList.svelte';
	import { money } from '#lib/expenses/labels.ts';

	let {
		expenses,
		phases = []
	}: {
		expenses: {
			entries: components['schemas']['Expense'][];
			categories: components['schemas']['ExpenseCategory'][];
			canWrite: boolean;
			me: string;
		};
		phases?: { value: string; label: string }[];
	} = $props();

	let open = $state(false);
	let cost = $derived(expenses.entries.reduce((t, e) => t + Number(e.amount), 0));
	let billable = $derived(expenses.entries.reduce((t, e) => t + Number(e.billAmount), 0));
	let currency = $derived(expenses.entries[0]?.currency ?? 'USD');
</script>

<Card title="Expenses">
	{#snippet actions()}
		{#if expenses.canWrite}
			<Button size="sm" onclick={() => (open = true)}><Plus size={14} /> Log</Button>
		{/if}
	{/snippet}
	{#if expenses.entries.length}
		<p class="mb-2 text-xs text-fg-subtle">
			{money(cost, currency)} spent{billable > 0 ? ` · ${money(billable, currency)} to bill` : ''}
		</p>
	{/if}
	<ExpenseList entries={expenses.entries} me={expenses.me} showWho />
</Card>

<ExpenseDialog bind:open categories={expenses.categories} {phases} />
