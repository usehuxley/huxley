<script lang="ts">
	import { resolve } from '$app/paths';
	import type { components } from '@usehuxley/api-client';

	type Project = components['schemas']['Project'];
	let { project }: { project: Project } = $props();

	const day = 86_400_000;
	const at = (d: string) => Date.parse(`${d}T00:00:00Z`);
	const done = (c: string) => c === 'resolved' || c === 'closed';
	// The span shown: the project's start to its last date, at least two weeks.
	let range = $derived.by(() => {
		const dates = [
			project.startOn,
			project.endOn,
			...project.tasks.flatMap((t) => [t.startOn, t.dueOn]),
			...project.milestones.map((m) => m.dueOn)
		].filter((d): d is string => !!d);
		const from = Math.min(...dates.map(at));
		const to = Math.max(from + 14 * day, ...dates.map(at)) + day;
		return { from, to };
	});
	const pct = (d: string) => ((at(d) - range.from) / (range.to - range.from)) * 100;
	let today = $derived(((Date.now() - range.from) / (range.to - range.from)) * 100);
	let weeks = $derived.by(() => {
		const out: { left: number; label: string }[] = [];
		for (let t = range.from; t < range.to; t += 7 * day) {
			out.push({
				left: ((t - range.from) / (range.to - range.from)) * 100,
				label: new Date(t).toLocaleDateString(undefined, {
					month: 'short',
					day: 'numeric',
					timeZone: 'UTC'
				})
			});
		}
		return out;
	});
	let dated = $derived(project.tasks.filter((t) => t.startOn || t.dueOn));
	let undated = $derived(project.tasks.length - dated.length);
</script>

<div class="flex flex-col gap-2" role="figure" aria-label="Timeline">
	<div class="grid grid-cols-[minmax(8rem,14rem)_1fr] gap-x-3 text-xs text-fg-muted">
		<span></span>
		<div class="relative h-5">
			{#each weeks as w (w.left)}
				<span class="absolute top-0 whitespace-nowrap" style="left: {w.left}%">{w.label}</span>
			{/each}
		</div>
	</div>
	<div class="grid grid-cols-[minmax(8rem,14rem)_1fr] items-center gap-x-3 gap-y-1.5 text-sm">
		<span class="text-fg-muted">Milestones</span>
		<div class="relative h-6 border-b border-border">
			{#each project.milestones.filter((m) => m.dueOn) as m (m.id)}
				<span
					class="absolute top-1 size-3 -translate-x-1/2 rotate-45 {m.reachedAt
						? 'bg-success'
						: 'bg-accent'}"
					style="left: {pct(m.dueOn!)}%"
					title="{m.name}: {m.dueOn}{m.reachedAt ? ' (reached)' : ''}"
					aria-label="{m.name}, due {m.dueOn}{m.reachedAt ? ', reached' : ''}"
					role="img"
				></span>
			{/each}
		</div>
		{#each dated as t (t.ticketId)}
			{@const start = t.startOn ?? t.dueOn!}
			{@const end = t.dueOn ?? t.startOn!}
			<a
				href={resolve('/(app)/tickets/[id]', { id: t.ticketId })}
				class="truncate text-accent hover:underline"
			>
				{t.title}
			</a>
			<div class="relative h-5">
				{#if today >= 0 && today <= 100}
					<span
						class="absolute inset-y-0 w-px bg-danger/60"
						style="left: {today}%"
						aria-hidden="true"
					></span>
				{/if}
				<span
					class="absolute inset-y-0.5 rounded {done(t.statusCategory)
						? 'bg-success/70'
						: t.waiting
							? 'bg-warning/70'
							: 'bg-accent/70'}"
					style="left: {pct(start)}%; width: max(0.5rem, {pct(end) -
						pct(start) +
						(day / (range.to - range.from)) * 100}%)"
					title="{t.title}: {start} to {end} · {t.assigneeName ?? 'Unassigned'} · {t.statusName}"
				></span>
			</div>
		{/each}
	</div>
	{#if undated > 0}
		<p class="text-xs text-fg-muted">
			{undated} task{undated === 1 ? '' : 's'} without dates aren’t shown: set them in the list.
		</p>
	{/if}
</div>
