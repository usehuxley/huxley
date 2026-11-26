<!--
	Lines (2px) or areas over time or categories. Split series stack as
	areas. A crosshair and tooltip follow the pointer.
-->
<script lang="ts">
	import { color, formatValue, maxOf, ticks, type Shaped } from '../data';
	import Legend from './Legend.svelte';

	let { data, area = false }: { data: Shaped; area?: boolean } = $props();
	let width = $state(480);
	let hover = $state<number | null>(null);

	const H = 200;
	const LEFT = 40;
	let stacked = $derived(area && data.split);
	let axis = $derived(ticks(maxOf(data, stacked)));
	let top = $derived(axis[axis.length - 1] || 1);
	let plotW = $derived(Math.max(60, width - LEFT - 8));
	let step = $derived(data.x.length > 1 ? plotW / (data.x.length - 1) : 0);
	const xAt = (i: number) => LEFT + (data.x.length > 1 ? i * step : plotW / 2);
	const yAt = (v: number) => H - (v / top) * (H - 10);

	/** Each series' points (stacked on those before when stacking). */
	let lines = $derived(
		data.series.map((s, si) => {
			const pts = data.x.map((_, i) => {
				const base = stacked
					? data.series.slice(0, si).reduce((a, p) => a + (p.values[i] ?? 0), 0)
					: 0;
				return { i, low: base, high: base + (s.values[i] ?? 0), missing: s.values[i] === null };
			});
			const line = pts
				.filter((p) => stacked || !p.missing)
				.map((p, k) => `${k === 0 ? 'M' : 'L'}${xAt(p.i)},${yAt(p.high)}`)
				.join(' ');
			const fill = area
				? `${line} ${[...pts]
						.reverse()
						.map((p) => `L${xAt(p.i)},${yAt(p.low)}`)
						.join(' ')} Z`
				: '';
			return { s, line, fill };
		})
	);

	function move(e: MouseEvent) {
		const box = (e.currentTarget as SVGElement).getBoundingClientRect();
		const x = e.clientX - box.left - LEFT;
		hover = data.x.length > 1 ? Math.max(0, Math.min(data.x.length - 1, Math.round(x / step))) : 0;
	}
	const fmt = (v: number) => formatValue(v, 'number');
</script>

<div class="relative flex flex-col gap-2" bind:clientWidth={width}>
	<Legend series={data.series} />
	<svg
		{width}
		height={H + 20}
		role="img"
		aria-label="{data.measureLabel} by {data.xLabel}"
		onmousemove={move}
		onmouseleave={() => (hover = null)}
	>
		{#each axis as t (t)}
			<line x1={LEFT} x2={LEFT + plotW} y1={yAt(t)} y2={yAt(t)} stroke="var(--viz-grid)" />
			<text x={LEFT - 4} y={yAt(t) + 4} text-anchor="end" class="fill-fg-muted text-[10px]">
				{fmt(t)}
			</text>
		{/each}
		{#each lines as { s, line, fill } (s.key)}
			{#if area}<path d={fill} fill={color(s.slot)} fill-opacity="0.25" />{/if}
			<path d={line} fill="none" stroke={color(s.slot)} stroke-width="2" stroke-linejoin="round" />
			{#if data.x.length <= 31}
				<!-- Markers, so a lone point (or a sparse series) still shows. -->
				{#each s.values as v, i (i)}
					{#if v !== null}
						<circle
							cx={xAt(i)}
							cy={yAt(
								stacked
									? data.series
											.slice(0, data.series.indexOf(s) + 1)
											.reduce((a, p) => a + (p.values[i] ?? 0), 0)
									: v
							)}
							r="4"
							fill={color(s.slot)}
							stroke="var(--viz-gap)"
							stroke-width="2"
						/>
					{/if}
				{/each}
			{/if}
		{/each}
		{#each data.x as label, i (label)}
			{#if data.x.length <= 8 || i % Math.ceil(data.x.length / 8) === 0}
				<text x={xAt(i)} y={H + 16} text-anchor="middle" class="fill-fg-muted text-[10px]">
					{label}
				</text>
			{/if}
		{/each}
		{#if hover !== null}
			<line x1={xAt(hover)} x2={xAt(hover)} y1="0" y2={H} stroke="var(--color-border-strong)" />
			{#each data.series as s (s.key)}
				{#if s.values[hover] !== null}
					<circle
						cx={xAt(hover)}
						cy={yAt(
							lines.find((l) => l.s.key === s.key) && stacked
								? data.series
										.slice(0, data.series.indexOf(s) + 1)
										.reduce((a, p) => a + (p.values[hover!] ?? 0), 0)
								: (s.values[hover] ?? 0)
						)}
						r="4"
						fill={color(s.slot)}
						stroke="var(--viz-gap)"
						stroke-width="2"
					/>
				{/if}
			{/each}
		{/if}
	</svg>
	{#if hover !== null}
		<div
			class="pointer-events-none absolute z-10 rounded-md border border-border bg-surface-raised px-2 py-1 text-xs shadow-card"
			style="left: {Math.min(xAt(hover) + 12, width - 160)}px; top: 24px"
			role="tooltip"
		>
			<p class="font-medium text-fg">{data.x[hover]}</p>
			{#each data.series as s (s.key)}
				{#if s.values[hover] !== null}<p class="text-fg-muted">
						{s.label}: {fmt(s.values[hover] ?? 0)}
					</p>{/if}
			{/each}
		</div>
	{/if}
</div>
