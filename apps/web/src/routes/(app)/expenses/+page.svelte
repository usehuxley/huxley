<script lang="ts">
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { enhance } from '$app/forms';
	import { Button, Card, Input, Select, toast } from '@usehuxley/ui';
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
	let ready = $derived(
		data.entries.filter(
			(e) => e.userId === data.me && (e.status === 'draft' || e.status === 'rejected')
		)
	);
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
			<div class="ml-auto flex gap-2">
				<Button href={resolve('/(app)/expenses/cards')}>Card charges</Button>
				<Button href={resolve('/(app)/expenses/reports')}>Reports</Button>
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

	{#if !data.everyone && ready.length > 0}
		<Card
			title="Submit for approval"
			description="Your drafts (and expenses sent back), as one report to your team lead."
		>
			<form
				method="POST"
				action="?/submitReport"
				class="flex flex-col gap-3"
				use:enhance={() =>
					async ({ result, update }) => {
						if (result.type === 'failure') {
							const failed = result.data as { message?: string } | undefined;
							toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
						}
						await update();
					}}
			>
				<fieldset class="flex flex-col gap-1">
					<legend class="sr-only">Expenses to submit</legend>
					{#each ready as e (e.id)}
						<label class="flex items-center gap-2 text-sm text-fg">
							<input type="checkbox" name="entryIds" value={e.id} checked />
							{e.description} · {money(e.amount, e.currency)}
						</label>
					{/each}
				</fieldset>
				<div class="flex flex-wrap items-end gap-2">
					<label class="text-sm text-fg">
						Title
						<Input
							name="title"
							placeholder="e.g. November, Acme trip"
							class="mt-1 w-64"
							maxlength={200}
						/>
					</label>
					<Button type="submit" variant="primary">Submit</Button>
				</div>
			</form>
		</Card>
	{/if}

	<Card>
		<ExpenseList entries={data.entries} me={data.me} showWho={data.everyone} showWhere />
	</Card>
</div>

<ExpenseDialog bind:open categories={data.categories} targets={data.targets} />
