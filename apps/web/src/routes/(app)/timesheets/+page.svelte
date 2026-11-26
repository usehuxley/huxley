<script lang="ts">
	import { resolve } from '$app/paths';
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import { formatDuration } from '#lib/tickets/duration.ts';
	import { STATUS } from '#lib/timesheets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const today = new Date().toISOString().slice(0, 10);
	let payFrom = $state(`${today.slice(0, 8)}01`);
	let payTo = $state(today);
	const saved: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({
					title: (result.data as { done?: string } | undefined)?.done ?? 'Saved.',
					tone: 'success'
				});
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const FILTERS = [
		{ value: 'submitted', label: 'To review' },
		{ value: 'rejected', label: 'Rejected' },
		{ value: 'approved', label: 'Approved' }
	];
</script>

<svelte:head>
	<title>Timesheets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Timesheets</h1>
		<p class="mt-1 text-sm text-fg-muted">
			The timesheets of the people you lead. Approving one locks its time.
		</p>
	</header>
	<nav aria-label="Timesheet status" class="flex gap-2">
		<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
		{#each FILTERS as f (f.value)}
			<a
				href="{resolve('/(app)/timesheets')}?status={f.value}"
				aria-current={data.status === f.value ? 'page' : undefined}
				class="rounded-control px-3 py-1.5 text-sm {data.status === f.value
					? 'bg-surface-sunken font-medium text-fg'
					: 'text-fg-muted hover:text-fg'}"
			>
				{f.label}
			</a>
		{/each}
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	</nav>
	<Card>
		{#if data.timesheets.length === 0}
			<EmptyState
				title={data.status === 'submitted' ? 'Nothing to review' : 'None'}
				description={data.status === 'submitted'
					? 'Submitted timesheets of people you lead show up here.'
					: ''}
			/>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Timesheets">
				{#each data.timesheets as t (t.id)}
					<li class="flex items-center justify-between gap-3 py-3 first:pt-0">
						<div>
							<a
								href={resolve('/(app)/timesheets/[id]', { id: t.id })}
								class="font-medium text-fg hover:underline"
							>
								{t.person}
							</a>
							<p class="text-xs text-fg-muted">
								{t.periodStart} to {t.periodEnd} · {formatDuration(t.totalMinutes) || 'nothing'}
							</p>
						</div>
						<Badge tone={STATUS[t.status]?.tone ?? 'neutral'}>
							{STATUS[t.status]?.label ?? t.status}
						</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
	{#if data.settings}
		<Card title="Settings" description="How periods are cut, and what billing previews use.">
			<form
				method="POST"
				action="?/settings"
				class="flex flex-wrap items-end gap-3"
				use:enhance={saved}
			>
				<div class="w-44">
					<Field label="Periods" for="ts-period">
						<Select
							id="ts-period"
							name="periodDays"
							value={String(data.settings.periodDays)}
							options={[
								{ value: '7', label: 'Weekly' },
								{ value: '14', label: 'Every two weeks' }
							]}
						/>
					</Field>
				</div>
				<div class="w-48">
					<Field label="A period starts on" for="ts-anchor">
						<Input id="ts-anchor" name="anchor" type="date" value={data.settings.anchor} />
					</Field>
				</div>
				<label class="flex items-center gap-2 pb-2 text-sm text-fg">
					<input type="checkbox" name="billApprovedOnly" checked={data.settings.billApprovedOnly} />
					Bill only approved time
				</label>
				<Button type="submit">Save</Button>
			</form>
		</Card>
		<Card title="Payroll export" description="Approved time and approved time off, as CSV.">
			<div class="flex flex-wrap items-end gap-3">
				<Field label="From" for="pay-from">
					<Input id="pay-from" type="date" bind:value={payFrom} />
				</Field>
				<Field label="To" for="pay-to"><Input id="pay-to" type="date" bind:value={payTo} /></Field>
				<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
				<Button
					href="{resolve('/(app)/timesheets')}/payroll.csv?from={payFrom}&to={payTo}"
					download
				>
					Download CSV
				</Button>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			</div>
		</Card>
	{/if}
</div>
