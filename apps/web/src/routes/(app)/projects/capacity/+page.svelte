<script lang="ts">
	import { resolve } from '$app/paths';
	import { ChevronLeft, ChevronRight, TriangleAlert } from '@lucide/svelte';
	import { Button, Card, EmptyState, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const day = 86_400_000;
	const base = resolve('/(app)/projects/capacity');
	// Six weeks earlier or later, keeping the team.
	const shifted = (weeks: number) => {
		const first = Date.parse(`${data.plan.weeks[0]}T00:00:00Z`) + weeks * 7 * day;
		const from = new Date(first).toISOString().slice(0, 10);
		return `${base}?from=${from}${data.team ? `&team=${data.team}` : ''}`;
	};
	const label = (d: string) =>
		new Date(`${d}T00:00:00Z`).toLocaleDateString(undefined, {
			month: 'short',
			day: 'numeric',
			timeZone: 'UTC'
		});
	const fill = (booked: number, capacity: number) =>
		capacity > 0 ? Math.min(100, (booked / capacity) * 100) : booked > 0 ? 100 : 0;
</script>

<svelte:head><title>Capacity · Projects · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(app)/projects')} class="text-sm text-accent hover:underline">Projects</a>
			<h1 class="mt-1 text-2xl font-semibold text-fg">Capacity</h1>
			<p class="text-fg-muted">
				Hours each person works a week (their schedule, less time off) against appointments and
				their project tasks’ remaining estimates, spread over the tasks’ dates.
			</p>
		</div>
		<form method="GET" action={base} class="flex items-end gap-2">
			<input type="hidden" name="from" value={data.plan.weeks[0]} />
			<div class="w-48">
				<Select
					aria-label="Team"
					name="team"
					value={data.team}
					options={[{ value: '', label: 'Everyone' }, ...data.teams]}
				/>
			</div>
			<Button type="submit" variant="ghost">Show</Button>
			<Button href={shifted(-6)} variant="ghost" aria-label="Earlier weeks">
				<ChevronLeft size={16} />
			</Button>
			<Button href={shifted(6)} variant="ghost" aria-label="Later weeks">
				<ChevronRight size={16} />
			</Button>
		</form>
	</div>
	{#if data.plan.people.length === 0}
		<EmptyState
			title="Nobody to plan"
			description="People appear here with a working schedule, project tasks or appointments."
		/>
	{:else}
		<Card>
			<div class="overflow-x-auto">
				<table class="w-full min-w-[40rem] text-sm">
					<caption class="sr-only">Booked hours against capacity, by person and week</caption>
					<thead>
						<tr class="text-left text-xs text-fg-muted">
							<th scope="col" class="py-2 pr-3 font-medium">Person</th>
							{#each data.plan.weeks as w (w)}<th scope="col" class="px-2 py-2 font-medium">
									Week of {label(w)}
								</th>{/each}
							<th scope="col" class="px-2 py-2 font-medium">Undated</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each data.plan.people as p (p.userId)}
							<tr>
								<th scope="row" class="py-2 pr-3 text-left font-medium text-fg">
									<a href={resolve('/(app)/people/[id]', { id: p.userId })} class="hover:underline">
										{p.name}
									</a>
									<span class="block text-xs font-normal text-fg-muted">
										{p.openTasks} open task{p.openTasks === 1 ? '' : 's'}
									</span>
								</th>
								{#each p.weeks as w, i (i)}
									<td class="px-2 py-2 align-top">
										<span
											class="flex items-center gap-1 tabular-nums {w.over
												? 'font-semibold text-danger'
												: 'text-fg'}"
										>
											{#if w.over}<TriangleAlert size={14} aria-hidden="true" />{/if}
											{w.bookedHours} / {w.capacityHours} h
											{#if w.over}<span class="sr-only">(over)</span>{/if}
										</span>
										<span class="mt-1 block h-1.5 rounded bg-surface-sunken" aria-hidden="true">
											<span
												class="block h-1.5 rounded {w.over ? 'bg-danger' : 'bg-accent'}"
												style="width: {fill(w.bookedHours, w.capacityHours)}%"
											></span>
										</span>
										<span class="mt-0.5 block text-xs text-fg-muted">
											{w.projectHours} project · {w.appointmentHours} appts
										</span>
									</td>
								{/each}
								<td class="px-2 py-2 align-top text-fg-muted tabular-nums">
									{p.unscheduledHours} h
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</Card>
	{/if}
</div>
