<!-- A client's health: the score, how it's moved, and what's behind it. -->
<script lang="ts">
	import { Badge, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { health }: { health: components['schemas']['ClientHealth'] } = $props();

	const tone = (s: number | null | undefined) =>
		s === null || s === undefined
			? 'neutral'
			: s >= 75
				? 'success'
				: s >= 50
					? 'warning'
					: 'danger';
	let points = $derived(
		health.history.filter((p): p is { day: string; score: number } => typeof p.score === 'number')
	);
	// A small trend line over the last 90 days (0 to 100).
	let path = $derived.by(() => {
		if (points.length < 2) return '';
		const w = 160;
		const h = 36;
		return points
			.map(
				(p, i) =>
					`${i === 0 ? 'M' : 'L'}${(i / (points.length - 1)) * w},${h - (p.score / 100) * h}`
			)
			.join(' ');
	});
</script>

<Card title="Health">
	<div class="flex flex-wrap items-center gap-4">
		<div class="flex items-baseline gap-2">
			<span class="text-3xl font-semibold text-fg tabular-nums">{health.score ?? '–'}</span>
			<span class="text-sm text-fg-muted">out of 100</span>
		</div>
		<Badge tone={tone(health.score)}>
			{health.score === null || health.score === undefined
				? 'Not enough data'
				: health.score >= 75
					? 'Healthy'
					: health.score >= 50
						? 'Needs attention'
						: 'At risk'}
		</Badge>
		{#if path}
			<svg
				width="160"
				height="36"
				role="img"
				aria-label="Health over the last 90 days"
				class="text-accent"
			>
				<path d={path} fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" />
			</svg>
		{/if}
	</div>
	{#if health.reasons.length > 0}
		<ul class="mt-3 flex flex-col gap-1 text-sm text-fg" aria-label="Why">
			{#each health.reasons as r (r)}<li>{r}</li>{/each}
		</ul>
	{/if}
	<details class="mt-3 text-sm">
		<summary class="cursor-pointer text-fg-muted">All factors</summary>
		<ul class="mt-2 flex flex-col gap-1" aria-label="Factors">
			{#each health.factors as f (f.key)}
				<li class="flex flex-wrap justify-between gap-2">
					<span class="text-fg">{f.label}</span>
					<span class="text-fg-muted">
						{f.score === null || f.score === undefined ? '–' : f.score} · {f.detail}
					</span>
				</li>
			{/each}
		</ul>
	</details>
	<p class="mt-2 text-xs text-fg-subtle">
		As of {new Date(`${health.computedOn}T00:00:00`).toLocaleDateString()}
	</p>
</Card>
