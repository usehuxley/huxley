<!--
	Bars from the baseline: horizontal for categories (their labels read
	well), vertical for time. Two groupings stack; several measures sit side
	by side. 2px surface gaps between fills; hover shows the values.
-->
<script lang="ts">
	import { color, formatValue, maxOf, ticks, type Shaped } from '../data';
	import Legend from './Legend.svelte';

	let { data }: { data: Shaped } = $props();
	let width = $state(480);
	let tip = $state<{ x: number; y: number; title: string; lines: string[] } | null>(null);

	let stacked = $derived(data.split);
	let vertical = $derived(data.xKind === 'date' || data.xKind === 'time');
	let axis = $derived(ticks(maxOf(data, stacked)));
	let top = $derived(axis[axis.length - 1] || 1);
	const LABEL = 120;
	const ROW = 28;
	let plotW = $derived(Math.max(60, width - (vertical ? 40 : LABEL + 12)));
	let height = $derived(vertical ? 220 : data.x.length * ROW + 24);

	function show(e: MouseEvent, i: number) {
		const box = (e.currentTarget as SVGElement).ownerSVGElement?.getBoundingClientRect();
		const lines = data.series
			.filter((s) => s.values[i] !== null)
			.map((s) => `${s.label}: ${formatValue(s.values[i], 'number')}`);
		tip = {
			x: e.clientX - (box?.left ?? 0),
			y: e.clientY - (box?.top ?? 0),
			title: data.x[i],
			lines
		};
	}
	const fmt = (v: number) => formatValue(v, 'number');
</script>

<div class="relative flex flex-col gap-2" bind:clientWidth={width}>
	<Legend series={data.series} />
	<svg
		{width}
		{height}
		role="img"
		aria-label="{data.measureLabel} by {data.xLabel}"
		class="overflow-visible"
	>
		{#if vertical}
			{@const band = plotW / Math.max(1, data.x.length)}
			{@const barW = Math.max(2, Math.min(40, band - 4))}
			{#each axis as t (t)}
				{@const y = 200 - (t / top) * 190}
				<line x1="36" x2={36 + plotW} y1={y} y2={y} stroke="var(--viz-grid)" stroke-width="1" />
				<text x="32" y={y + 4} text-anchor="end" class="fill-fg-muted text-[10px]">{fmt(t)}</text>
			{/each}
			{#each data.x as label, i (label)}
				{@const x0 = 36 + i * band + (band - barW) / 2}
				{@const parts = data.series.map((s) => ({ s, v: s.values[i] ?? 0 }))}
				<g role="presentation" onmousemove={(e) => show(e, i)} onmouseleave={() => (tip = null)}>
					<rect x={36 + i * band} y="0" width={band} height="200" fill="transparent" />
					{#each parts as { s, v }, j (s.key)}
						{@const below = parts.slice(0, j).reduce((a, p) => a + p.v, 0)}
						{@const h = (v / top) * 190}
						{#if v > 0}
							<rect
								x={x0}
								y={200 - ((below + v) / top) * 190}
								width={barW}
								height={Math.max(0, h - (j > 0 ? 2 : 0))}
								rx="3"
								fill={color(s.slot)}
							/>
						{/if}
					{/each}
				</g>
				{#if data.x.length <= 12 || i % Math.ceil(data.x.length / 12) === 0}
					<text x={x0 + barW / 2} y="214" text-anchor="middle" class="fill-fg-muted text-[10px]">
						{label}
					</text>
				{/if}
			{/each}
		{:else}
			{#each data.x as label, i (label)}
				{@const y = i * ROW}
				{@const parts = data.series.map((s) => ({ s, v: s.values[i] ?? 0 }))}
				<g role="presentation" onmousemove={(e) => show(e, i)} onmouseleave={() => (tip = null)}>
					<rect x="0" {y} {width} height={ROW} fill="transparent" />
					<text x={LABEL - 6} y={y + ROW / 2 + 4} text-anchor="end" class="fill-fg text-xs">
						{label.length > 18 ? `${label.slice(0, 17)}…` : label}
					</text>
					{#if stacked}
						{#each parts as { s, v }, j (s.key)}
							{@const before = parts.slice(0, j).reduce((a, p) => a + p.v, 0)}
							{#if v > 0}
								<rect
									x={LABEL + (before / top) * plotW + (j > 0 ? 2 : 0)}
									y={y + 6}
									width={Math.max(1, (v / top) * plotW - (j > 0 ? 2 : 0))}
									height={ROW - 12}
									rx="3"
									fill={color(s.slot)}
								/>
							{/if}
						{/each}
					{:else}
						{@const sub = (ROW - 12) / parts.length}
						{#each parts as { s, v }, j (s.key)}
							<rect
								x={LABEL}
								y={y + 6 + j * sub}
								width={Math.max(v > 0 ? 2 : 0, (v / top) * plotW)}
								height={Math.max(2, sub - 2)}
								rx="3"
								fill={color(s.slot)}
							/>
						{/each}
						{#if parts.length === 1}
							<text
								x={LABEL + (parts[0].v / top) * plotW + 6}
								y={y + ROW / 2 + 4}
								class="fill-fg-muted text-[11px] tabular-nums"
							>
								{fmt(parts[0].v)}
							</text>
						{/if}
					{/if}
				</g>
			{/each}
		{/if}
	</svg>
	{#if tip}
		<div
			class="pointer-events-none absolute z-10 rounded-md border border-border bg-surface-raised px-2 py-1 text-xs shadow-card"
			style="left: {Math.min(tip.x + 12, width - 160)}px; top: {tip.y + 12}px"
			role="tooltip"
		>
			<p class="font-medium text-fg">{tip.title}</p>
			{#each tip.lines as line (line)}<p class="text-fg-muted">{line}</p>{/each}
		</div>
	{/if}
</div>
