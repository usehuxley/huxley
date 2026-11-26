<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import { formatDuration } from '#lib/tickets/duration.ts';
	import { STATUS } from '#lib/timesheets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let sheet = $derived(data.sheet);
	let note = $state('');
	let mine = $derived(sheet.userId === data.user.id);

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({
					title: (result.data as { done?: string } | undefined)?.done ?? 'Done.',
					tone: 'success'
				});
				note = '';
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>{sheet.person}’s timesheet · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(app)/timesheets')} class="text-sm text-accent hover:underline">
				Timesheets
			</a>
			<h1 class="text-2xl font-semibold text-fg">{sheet.person}’s timesheet</h1>
			<p class="mt-1 text-sm text-fg-muted">
				{sheet.periodStart} to {sheet.periodEnd}: {formatDuration(sheet.totalMinutes) || 'nothing'}
				logged, {formatDuration(sheet.billableMinutes) || 'none'} billable{sheet.expectedMinutes !==
					null && sheet.expectedMinutes !== undefined
					? `, ${formatDuration(sheet.expectedMinutes) || '0m'} expected`
					: ''}.
			</p>
		</div>
		<Badge tone={STATUS[sheet.status]?.tone ?? 'neutral'}>
			{STATUS[sheet.status]?.label ?? sheet.status}
		</Badge>
	</header>

	<Card title="Days">
		<table class="w-full text-sm" aria-label="Days">
			<thead class="text-left text-fg-muted">
				<tr>
					<th class="py-1 font-medium">Day</th>
					<th class="py-1 text-right font-medium">Logged</th>
					<th class="py-1 text-right font-medium">Billable</th>
					<th class="py-1 text-right font-medium">Expected</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-border">
				{#each sheet.days as d (d.date)}
					<tr>
						<td class="py-1">{d.date}{d.timeOff ? ` (${d.timeOff})` : ''}</td>
						<td class="py-1 text-right tabular-nums">{formatDuration(d.minutes) || '–'}</td>
						<td class="py-1 text-right tabular-nums">{formatDuration(d.billableMinutes) || '–'}</td>
						<td class="py-1 text-right tabular-nums">
							{d.expectedMinutes === null || d.expectedMinutes === undefined
								? '–'
								: formatDuration(d.expectedMinutes) || '0m'}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</Card>

	<Card
		title="Entries"
		description={sheet.missingNotes > 0 ? `${sheet.missingNotes} without notes` : undefined}
	>
		<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Entries">
			{#each sheet.entries as e (e.id)}
				<li class="flex justify-between gap-3 py-2">
					<span>
						{e.workedOn} · {e.workTypeName}{e.ticketReference ? ` · ${e.ticketReference}` : ''}
						<span class="block text-xs text-fg-muted">{e.notes || 'No notes'}</span>
					</span>
					<span class="tabular-nums">
						{formatDuration(e.minutes)}{e.billable ? '' : ' (not billable)'}
					</span>
				</li>
			{:else}
				<li class="py-2 text-fg-muted">No time logged.</li>
			{/each}
		</ul>
	</Card>

	{#if sheet.history.length > 0}
		<Card title="History">
			<ul class="flex flex-col gap-1 text-sm" aria-label="Timesheet history">
				{#each sheet.history as h, i (i)}
					<li class="text-fg-muted">
						<span class="text-fg">{STATUS[h.action]?.label ?? h.action}</span>
						by {h.by ?? 'someone'}, {new Date(h.at).toLocaleString()}{h.note ? `: “${h.note}”` : ''}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if !mine && (sheet.status === 'submitted' || sheet.status === 'approved')}
		<Card title={sheet.status === 'submitted' ? 'Review' : 'Reopen'}>
			<div class="flex flex-col gap-3">
				<Field
					label="Comment"
					for="review-note"
					hint={sheet.status === 'submitted'
						? 'Needed to reject: say what needs changing'
						: 'Say why it’s being reopened'}
				>
					<Input id="review-note" bind:value={note} maxlength={2000} />
				</Field>
				<div class="flex flex-wrap gap-2">
					{#if sheet.status === 'submitted'}
						<form method="POST" action="?/decide" use:enhance={done}>
							<input type="hidden" name="approve" value="true" />
							<input type="hidden" name="note" value={note} />
							<Button type="submit">Approve</Button>
						</form>
						<form method="POST" action="?/decide" use:enhance={done}>
							<input type="hidden" name="approve" value="false" />
							<input type="hidden" name="note" value={note} />
							<Button type="submit" variant="danger">Reject</Button>
						</form>
					{/if}
					<form method="POST" action="?/reopen" use:enhance={done}>
						<input type="hidden" name="note" value={note} />
						<Button type="submit" variant="secondary">Reopen</Button>
					</form>
				</div>
			</div>
		</Card>
	{/if}
</div>
