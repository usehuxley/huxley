<script lang="ts">
	import { resolve } from '$app/paths';
	import type { components } from '@usehuxley/api-client';
	import { ChevronLeft, ChevronRight } from '@lucide/svelte';
	import { Button } from '@usehuxley/ui';

	type Project = components['schemas']['Project'];
	let { project, month }: { project: Project; month: string | null } = $props();

	const pad = (n: number) => String(n).padStart(2, '0');
	const iso = (d: Date) =>
		`${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
	const done = (c: string) => c === 'resolved' || c === 'closed';

	// The month shown: asked for, else this month if the project spans it,
	// else the month of its first date.
	let first = $derived.by(() => {
		if (month && /^\d{4}-\d{2}$/.test(month)) return new Date(`${month}-01T00:00:00Z`);
		const today = iso(new Date());
		const dates = [
			project.startOn,
			project.endOn,
			...project.tasks.flatMap((t) => [t.startOn, t.dueOn])
		].filter((d): d is string => !!d);
		const end = dates.reduce((a, b) => (b > a ? b : a), project.startOn);
		const pick = today >= project.startOn && today <= end ? today : project.startOn;
		return new Date(`${pick.slice(0, 7)}-01T00:00:00Z`);
	});
	let label = $derived(
		first.toLocaleDateString(undefined, { month: 'long', year: 'numeric', timeZone: 'UTC' })
	);
	const DAY = 86_400_000;
	// Months and days by arithmetic on UTC timestamps (no Date is changed).
	const shift = (by: number) => {
		const y = first.getUTCFullYear();
		const m = first.getUTCMonth() + by;
		const target = new Date(Date.UTC(y, m, 1));
		return `${resolve('/(app)/projects/[id]', { id: project.id })}?view=calendar&month=${iso(target).slice(0, 7)}`;
	};
	// Whole weeks, Monday first, covering the month.
	let days = $derived.by(() => {
		const startMs = first.getTime() - ((first.getUTCDay() + 6) % 7) * DAY;
		const monthEnd = Date.UTC(first.getUTCFullYear(), first.getUTCMonth() + 1, 1);
		const weeks = Math.ceil((monthEnd - startMs) / (7 * DAY));
		return Array.from({ length: weeks * 7 }, (_, n) => {
			const d = new Date(startMs + n * DAY);
			return { day: iso(d), inMonth: d.getUTCMonth() === first.getUTCMonth() };
		});
	});
	const on = (day: string) => ({
		milestones: project.milestones.filter((m) => m.dueOn === day),
		due: project.tasks.filter((t) => t.dueOn === day),
		starts: project.tasks.filter((t) => t.startOn === day && t.startOn !== t.dueOn)
	});
	const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const today = iso(new Date());
</script>

<div class="flex flex-col gap-2">
	<div class="flex items-center justify-between">
		<h2 class="text-sm font-semibold text-fg">{label}</h2>
		<div class="flex gap-1">
			<Button href={shift(-1)} variant="ghost" size="sm" aria-label="Previous month">
				<ChevronLeft size={16} />
			</Button>
			<Button href={shift(1)} variant="ghost" size="sm" aria-label="Next month">
				<ChevronRight size={16} />
			</Button>
		</div>
	</div>
	<div class="overflow-x-auto">
		<table class="w-full min-w-[42rem] table-fixed border-collapse text-xs">
			<caption class="sr-only">{project.name}: milestones and task dates in {label}</caption>
			<thead>
				<tr>
					{#each WEEKDAYS as w (w)}<th scope="col" class="py-1 text-left font-medium text-fg-muted">
							{w}
						</th>{/each}
				</tr>
			</thead>
			<tbody>
				{#each Array.from( { length: Math.ceil(days.length / 7) }, (_, i) => days.slice(i * 7, i * 7 + 7) ) as week, i (i)}
					<tr>
						{#each week as d (d.day)}
							{@const items = on(d.day)}
							<td
								class="h-24 border border-border p-1 align-top {d.inMonth
									? ''
									: 'bg-surface-sunken'}"
							>
								<span
									class="tabular-nums {d.day === today ? 'font-bold text-accent' : 'text-fg-muted'}"
								>
									{Number(d.day.slice(8))}
								</span>
								<ul class="mt-0.5 flex flex-col gap-0.5" aria-label="On {d.day}">
									{#each items.milestones as m (m.id)}
										<li
											class="truncate rounded px-1 {m.reachedAt
												? 'bg-success-soft text-success'
												: 'bg-accent-soft text-fg'}"
										>
											◆ {m.name}
										</li>
									{/each}
									{#each items.starts as t (t.ticketId)}
										<li class="truncate">
											<a
												href={resolve('/(app)/tickets/[id]', { id: t.ticketId })}
												class="text-fg-muted hover:underline"
											>
												▸ {t.title}
											</a>
										</li>
									{/each}
									{#each items.due as t (t.ticketId)}
										<li class="truncate">
											<a
												href={resolve('/(app)/tickets/[id]', { id: t.ticketId })}
												class="hover:underline {done(t.statusCategory)
													? 'text-fg-muted line-through'
													: 'text-fg'}"
											>
												Due: {t.title}
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
	</div>
	<p class="text-xs text-fg-muted">◆ milestone · ▸ task starts · Due: task due</p>
</div>
