<!--
	A donut for one measure by one grouping: up to seven slices keep their
	colors, the rest fold into "Other". Labeled with values and shares.
-->
<script lang="ts">
	import { color, formatValue, MAX_SERIES, type Shaped } from '../data';

	let { data }: { data: Shaped } = $props();
	let hover = $state<number | null>(null);

	let slices = $derived.by(() => {
		const values = data.series[0]?.values ?? [];
		const all = data.x
			.map((label, i) => ({ label, value: values[i] ?? 0 }))
			.filter((s) => s.value > 0)
			.sort((a, b) => b.value - a.value);
		const kept = all.length > MAX_SERIES ? all.slice(0, MAX_SERIES - 1) : all;
		const rest = all.slice(kept.length).reduce((a, s) => a + s.value, 0);
		const list = kept.map((s, i) => ({ ...s, slot: i + 1 }));
		if (rest > 0) list.push({ label: 'Other', value: rest, slot: 0 });
		const total = list.reduce((a, s) => a + s.value, 0) || 1;
		let angle = -Math.PI / 2;
		return list.map((s) => {
			const sweep = (s.value / total) * Math.PI * 2;
			const start = angle;
			angle += sweep;
			return { ...s, share: s.value / total, start, end: angle };
		});
	});

	const R = 70;
	const r = 44;
	function arc(start: number, end: number): string {
		if (end - start >= Math.PI * 2 - 1e-6) end = start + Math.PI * 2 - 1e-4;
		const large = end - start > Math.PI ? 1 : 0;
		const p = (rad: number, a: number) => `${80 + rad * Math.cos(a)},${80 + rad * Math.sin(a)}`;
		return `M${p(R, start)} A${R},${R} 0 ${large} 1 ${p(R, end)} L${p(r, end)} A${r},${r} 0 ${large} 0 ${p(r, start)} Z`;
	}
	const pct = (v: number) => `${Math.round(v * 100)}%`;
</script>

<div class="flex flex-wrap items-center gap-6">
	<svg width="160" height="160" role="img" aria-label="{data.measureLabel} by {data.xLabel}">
		{#each slices as s, i (s.label)}
			<path
				d={arc(s.start, s.end)}
				fill={color(s.slot)}
				stroke="var(--viz-gap)"
				stroke-width="2"
				opacity={hover === null || hover === i ? 1 : 0.5}
				role="presentation"
				onmouseenter={() => (hover = i)}
				onmouseleave={() => (hover = null)}
			/>
		{/each}
		{#if hover !== null}
			<text x="80" y="84" text-anchor="middle" class="fill-fg text-sm font-semibold">
				{pct(slices[hover].share)}
			</text>
		{/if}
	</svg>
	<ul class="flex flex-col gap-1 text-sm" aria-label="Legend">
		{#each slices as s (s.label)}
			<li class="flex items-center gap-2">
				<span class="inline-block size-2.5 rounded-sm" style="background: {color(s.slot)}"></span>
				<span class="text-fg">{s.label}</span>
				<span class="text-fg-muted tabular-nums">
					{formatValue(s.value, 'number')} · {pct(s.share)}
				</span>
			</li>
		{/each}
	</ul>
</div>
