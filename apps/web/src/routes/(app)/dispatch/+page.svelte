<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { ChevronLeft, ChevronRight } from '@lucide/svelte';
	import { Button, EmptyState, Select } from '@usehuxley/ui';
	import { addWeeks, isoDay } from '#lib/time/week.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const WEEKDAYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
	const today = isoDay(new Date());
	const clock = (iso: string) =>
		new Date(iso).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	const dayLabel = (day: string) =>
		new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
			weekday: 'short',
			day: 'numeric'
		});

	type Person = (typeof data.people)[number];
	/** What a person has on a day (in this browser's dates). */
	function cell(person: Person, day: string) {
		const away = person.timeOff.find((t) => t.startsOn <= day && day <= t.endsOn);
		const weekday = WEEKDAYS[new Date(`${day}T12:00:00`).getDay()];
		const hours = person.hours;
		const ranges = hours ? (hours[weekday] ?? []) : null;
		return {
			away: away?.kind ?? null,
			off: ranges !== null && ranges.length === 0,
			hours: ranges?.map(([a, b]) => `${a}–${b}`).join(', ') ?? '',
			appointments: person.appointments.filter((a) => isoDay(new Date(a.startsAt)) === day)
		};
	}

	function week(monday: string, queue: string): string {
		const q = [`week=${monday}`, queue ? `queue=${queue}` : ''].filter(Boolean).join('&');
		return `${resolve('/(app)/dispatch')}?${q}`;
	}
</script>

<svelte:head>
	<title>Dispatch · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-7xl flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Dispatch</h1>
			<p class="mt-1 text-fg-muted">Who's booked, away or off, for the week.</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			{#if data.queues.length > 0}
				<div class="w-48">
					<label for="dispatch-queue" class="sr-only">Queue</label>
					<Select
						id="dispatch-queue"
						value={data.queueId}
						options={[
							{ value: '', label: 'Everyone in a queue' },
							...data.queues.map((q) => ({ value: q.id, label: q.name }))
						]}
						onchange={(e: Event) =>
							// eslint-disable-next-line svelte/no-navigation-without-resolve -- week() resolves
							void goto(week(data.monday, (e.currentTarget as HTMLSelectElement).value))}
					/>
				</div>
			{/if}
			<nav aria-label="Weeks" class="flex items-center gap-2">
				<!-- eslint-disable svelte/no-navigation-without-resolve -- week() resolves -->
				<Button href={week(addWeeks(data.monday, -1), data.queueId)} aria-label="Previous week">
					<ChevronLeft size={16} />
				</Button>
				<Button href={week(addWeeks(data.monday, 1), data.queueId)} aria-label="Next week">
					<ChevronRight size={16} />
				</Button>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			</nav>
		</div>
	</div>

	{#if data.people.length === 0}
		<EmptyState
			title="Nobody to show"
			description="People appear here once they're members of a queue, or booked on a ticket."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full min-w-[56rem] table-fixed text-left text-sm" aria-label="Dispatch board">
				<thead class="border-b border-border bg-surface-sunken text-xs text-fg-muted">
					<tr>
						<th scope="col" class="w-40 px-3 py-2 font-medium">Person</th>
						{#each data.days as day (day)}
							<th scope="col" class="px-2 py-2 font-medium {day === today ? 'text-accent' : ''}">
								{dayLabel(day)}
							</th>
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each data.people as person (person.userId)}
						<tr class="border-b border-border align-top last:border-0">
							<th scope="row" class="px-3 py-2 font-medium text-fg">{person.name}</th>
							{#each data.days as day (day)}
								{@const c = cell(person, day)}
								<td class="px-2 py-2 {c.away || c.off ? 'bg-surface-sunken' : ''}">
									{#if c.away}
										<p class="text-xs text-fg-muted">Away ({c.away})</p>
									{:else if c.off}
										<p class="text-xs text-fg-subtle">Off</p>
									{:else if c.hours}
										<p class="text-xs text-fg-subtle">{c.hours}</p>
									{/if}
									<ul class="mt-1 flex flex-col gap-1">
										{#each c.appointments as a (a.id)}
											<li>
												<a
													href={resolve('/(app)/tickets/[id]', { id: a.ticketId })}
													class="block rounded-control bg-accent-soft px-1.5 py-1 text-xs text-fg hover:underline"
													title={`${a.ticketTitle} · ${a.orgName}${a.notes ? ` · ${a.notes}` : ''}`}
												>
													<span class="tabular-nums">{clock(a.startsAt)}–{clock(a.endsAt)}</span>
													<span class="block truncate">{a.ticketReference} {a.ticketTitle}</span>
												</a>
											</li>
										{/each}
									</ul>
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
