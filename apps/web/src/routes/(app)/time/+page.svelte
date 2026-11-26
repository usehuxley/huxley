<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { ChevronLeft, ChevronRight, Play, Square, X } from '@lucide/svelte';
	import { Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import { formatDuration } from '#lib/tickets/duration.ts';
	import { addWeeks, isoDay } from '#lib/time/week.ts';
	import { clock, timers } from '#lib/time/timer.svelte.ts';
	import TimesheetCard from '#lib/timesheets/TimesheetCard.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	const today = isoDay(new Date());
	let errors: Record<string, string> = $derived(
		(form as { errors?: Record<string, string> } | null)?.errors ?? {}
	);
	let byDay = $derived(
		data.days.map((day) => ({
			day,
			entries: data.entries.filter((e) => e.workedOn === day),
			minutes: data.entries.filter((e) => e.workedOn === day).reduce((n, e) => n + e.minutes, 0)
		}))
	);
	let total = $derived(data.entries.reduce((n, e) => n + e.minutes, 0));
	let billable = $derived(
		data.entries.filter((e) => e.billable).reduce((n, e) => n + e.minutes, 0)
	);

	let workTypeId = $state('');
	let isBillable = $state(false);
	let duration = $state('');
	$effect(() => {
		// Non-ticket work first.
		if (!data.workTypes.some((w) => w.id === workTypeId)) {
			workTypeId = (data.workTypes.find((w) => !w.onTickets) ?? data.workTypes[0])?.id ?? '';
		}
	});
	$effect(() => {
		const kind = data.workTypes.find((w) => w.id === workTypeId);
		if (kind) isBillable = kind.billable;
	});
	onMount(() => timers.init());

	const dayName = (day: string) =>
		new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
			weekday: 'long',
			day: 'numeric',
			month: 'short'
		});

	const done =
		(message: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update();
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				duration = '';
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>My time · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">My time</h1>
			<p class="mt-1 text-fg-muted">
				{formatDuration(total) || 'Nothing'} this week{billable > 0
					? `, ${formatDuration(billable)} billable`
					: ''}.
			</p>
		</div>
		<nav aria-label="Weeks" class="flex items-center gap-2">
			<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
			<Button
				href={resolve('/(app)/time') + `?week=${addWeeks(data.monday, -1)}`}
				aria-label="Previous week"
			>
				<ChevronLeft size={16} />
			</Button>
			<span class="text-sm text-fg">Week of {dayName(data.monday)}</span>
			<Button
				href={resolve('/(app)/time') + `?week=${addWeeks(data.monday, 1)}`}
				aria-label="Next week"
			>
				<ChevronRight size={16} />
			</Button>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		</nav>
	</div>

	{#if data.timesheet}
		<TimesheetCard
			sheet={data.timesheet}
			query="week={data.monday}"
			canSubmit={data.timesheet.periodStart <= today}
		/>
	{/if}

	<Card
		title="Log time"
		description="Time that isn't on a ticket: admin, meetings, training, travel."
	>
		<form
			method="POST"
			action="?/log"
			class="grid gap-3 sm:grid-cols-[8rem_10rem_1fr]"
			use:enhance={done('Time logged.')}
		>
			<Field label="Time spent" for="time-duration" error={errors.minutes}>
				<Input
					id="time-duration"
					name="duration"
					required
					placeholder="e.g. 1h 30m"
					bind:value={duration}
				/>
			</Field>
			<Field label="Day" for="time-day" error={errors.workedOn}>
				<Input
					id="time-day"
					name="workedOn"
					type="date"
					value={data.days.includes(today) ? today : data.monday}
				/>
			</Field>
			<Field label="Work type" for="time-type" error={errors.workTypeId}>
				<Select
					id="time-type"
					name="workTypeId"
					bind:value={workTypeId}
					options={data.workTypes.map((w) => ({ value: w.id, label: w.name }))}
				/>
			</Field>
			<Field label="Notes" for="time-notes" class="sm:col-span-3">
				<Input id="time-notes" name="notes" maxlength={4000} />
			</Field>
			<div class="flex flex-wrap items-center justify-between gap-3 sm:col-span-3">
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="billable" bind:checked={isBillable} />
					Billable
				</label>
				<div class="flex items-center gap-2">
					{#if timers.running('general')}
						<span class="text-sm text-fg tabular-nums" aria-live="polite">
							{clock(timers.elapsed('general'))}
						</span>
						<Button
							onclick={() => (duration = formatDuration(timers.stop('general')))}
							aria-label="Stop the timer"
						>
							<Square size={14} /> Stop
						</Button>
					{:else}
						<Button onclick={() => timers.start('general')}><Play size={14} /> Start timer</Button>
					{/if}
					<Button type="submit" variant="primary">Log time</Button>
				</div>
			</div>
		</form>
	</Card>

	<section aria-label="This week" class="flex flex-col gap-3">
		{#each byDay as d (d.day)}
			<div class="rounded-card border border-border bg-surface">
				<div class="flex items-center justify-between border-b border-border px-4 py-2">
					<h2 class="text-sm font-medium text-fg {d.day === today ? 'text-accent' : ''}">
						{dayName(d.day)}
					</h2>
					<span class="text-sm text-fg-muted tabular-nums">{formatDuration(d.minutes) || '–'}</span>
				</div>
				{#if d.entries.length > 0}
					<ul class="flex flex-col divide-y divide-border" aria-label={dayName(d.day)}>
						{#each d.entries as entry (entry.id)}
							<li class="flex items-start justify-between gap-3 px-4 py-2 text-sm">
								<div class="min-w-0">
									<p class="text-fg">
										{formatDuration(entry.minutes)} · {entry.workTypeName}
										{#if !entry.billable}<span class="text-xs text-fg-muted">
												(not billable)
											</span>{/if}
									</p>
									<p class="truncate text-xs text-fg-muted">
										{#if entry.ticketId}
											<a
												href={resolve('/(app)/tickets/[id]', { id: entry.ticketId })}
												class="hover:text-fg hover:underline"
											>
												{entry.ticketReference}
												{entry.ticketTitle}
											</a>{entry.notes ? ' · ' : ''}
										{/if}{entry.notes}
									</p>
								</div>
								<form method="POST" action="?/delete" use:enhance={done('Time deleted.')}>
									<input type="hidden" name="id" value={entry.id} />
									<button
										type="submit"
										class="rounded-control p-1 text-fg-muted hover:bg-surface-sunken hover:text-fg"
										aria-label={`Delete ${formatDuration(entry.minutes)} ${entry.workTypeName}`}
									>
										<X size={14} />
									</button>
								</form>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/each}
	</section>
</div>
