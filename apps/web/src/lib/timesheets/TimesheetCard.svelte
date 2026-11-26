<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { formatDuration } from '#lib/tickets/duration.ts';
	import { STATUS } from './status.ts';

	type Timesheet = components['schemas']['Timesheet'];

	type Props = {
		sheet: Timesheet;
		/** The page's query string, kept when posting (e.g. `week=2026-11-23`). */
		query: string;
		/** Whether you can submit (the period has started). */
		canSubmit: boolean;
	};
	let { sheet, query, canSubmit }: Props = $props();
	let note = $state('');

	const short = (day: string) =>
		new Date(`${day}T12:00:00`).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' });

	const done =
		(message: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				note = '';
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<Card
	title="Timesheet"
	description="{sheet.periodStart} to {sheet.periodEnd}: {formatDuration(sheet.totalMinutes) ||
		'nothing'} logged{sheet.expectedMinutes !== null && sheet.expectedMinutes !== undefined
		? ` of ${formatDuration(sheet.expectedMinutes) || '0m'} expected`
		: ''}."
>
	{#snippet actions()}
		<Badge tone={STATUS[sheet.status]?.tone ?? 'neutral'}>
			{STATUS[sheet.status]?.label ?? sheet.status}
		</Badge>
	{/snippet}
	<ul class="grid grid-cols-7 gap-1 text-center text-xs" aria-label="Days of the timesheet">
		{#each sheet.days as d (d.date)}
			{@const short_ =
				d.expectedMinutes !== null &&
				d.expectedMinutes !== undefined &&
				d.minutes < d.expectedMinutes}
			<li class="rounded-md border border-border px-1 py-2 {short_ ? 'border-warning' : ''}">
				<p class="text-fg-muted">{short(d.date)}</p>
				<p class="font-medium text-fg">{formatDuration(d.minutes) || '–'}</p>
				{#if d.timeOff}<p class="text-fg-subtle">{d.timeOff}</p>{:else if d.expectedMinutes}<p
						class="text-fg-subtle"
					>
						of {formatDuration(d.expectedMinutes)}
					</p>{/if}
			</li>
		{/each}
	</ul>
	{#if sheet.missingNotes > 0}
		<p class="mt-2 text-sm text-warning">
			{sheet.missingNotes}
			{sheet.missingNotes === 1 ? 'entry has' : 'entries have'} no notes.
		</p>
	{/if}
	{#if sheet.history.length > 0}
		<ul class="mt-3 flex flex-col gap-1 text-sm" aria-label="Timesheet history">
			{#each sheet.history as h, i (i)}
				<li class="text-fg-muted">
					<span class="text-fg">{STATUS[h.action]?.label ?? h.action}</span>
					by {h.by ?? 'someone'}, {new Date(h.at).toLocaleString()}{h.note ? `: “${h.note}”` : ''}
				</li>
			{/each}
		</ul>
	{/if}
	{#if canSubmit && (sheet.status === 'open' || sheet.status === 'rejected')}
		<form
			method="POST"
			action="?{query}&/submitTimesheet"
			class="mt-4 flex flex-wrap items-end gap-3"
			use:enhance={done('Timesheet submitted.')}
		>
			<input type="hidden" name="periodStart" value={sheet.periodStart} />
			<div class="min-w-64 flex-1">
				<Field label="Note for your lead" for="ts-note">
					<Input id="ts-note" name="note" bind:value={note} maxlength={2000} />
				</Field>
			</div>
			<Button type="submit">{sheet.status === 'rejected' ? 'Resubmit' : 'Submit timesheet'}</Button>
		</form>
	{:else if sheet.status === 'submitted' && sheet.id}
		<form
			method="POST"
			action="?{query}&/withdrawTimesheet"
			class="mt-4"
			use:enhance={done('Timesheet withdrawn.')}
		>
			<input type="hidden" name="id" value={sheet.id} />
			<Button type="submit" variant="secondary">Withdraw to change it</Button>
		</form>
	{:else if sheet.status === 'approved'}
		<p class="mt-3 text-sm text-fg-muted">Approved: its time is locked. Your lead can reopen it.</p>
	{/if}
</Card>
