<!--
	A dashboard widget: a title, and its answer as a number, a table or a
	chart (with a table view of the same numbers).
-->
<script lang="ts">
	import { shape, type Answer, type Viz } from './data';
	import BarChart from './charts/BarChart.svelte';
	import LineChart from './charts/LineChart.svelte';
	import NumberView from './charts/NumberView.svelte';
	import PieChart from './charts/PieChart.svelte';
	import TableView from './charts/TableView.svelte';

	type Props = {
		title: string;
		viz: Viz;
		answer?: Answer | null;
		/** Why there's no answer, e.g. "You can't see this data". */
		problem?: string | null;
		width?: number;
	};
	let { title, viz, answer = null, problem = null, width = 6 }: Props = $props();
	let asTable = $state(false);
	let shaped = $derived(answer && viz !== 'number' && viz !== 'table' ? shape(answer) : null);
</script>

<section
	class="hx-viz col-span-full flex min-w-0 flex-col gap-3 rounded-card border border-border bg-surface p-4 shadow-card md:col-span-(--w)"
	style="--w: {width}"
	aria-label={title}
>
	<header class="flex items-start justify-between gap-2">
		<h3 class="text-sm font-medium text-fg-muted">{title}</h3>
		{#if shaped && answer && answer.rows.length > 0}
			<button
				type="button"
				class="text-xs text-fg-muted hover:text-fg"
				aria-pressed={asTable}
				onclick={() => (asTable = !asTable)}
			>
				{asTable ? 'Chart' : 'Table'}
			</button>
		{/if}
	</header>
	{#if problem}
		<p class="text-sm text-fg-muted">{problem}</p>
	{:else if !answer}
		<p class="text-sm text-fg-muted">Loading…</p>
	{:else if viz === 'number'}
		<NumberView {answer} />
	{:else if answer.rows.length === 0}
		<p class="text-sm text-fg-muted">Nothing in this range.</p>
	{:else if viz === 'table' || asTable}
		<TableView {answer} label={title} />
	{:else if shaped && viz === 'pie'}
		<PieChart data={shaped} />
	{:else if shaped && (viz === 'line' || viz === 'area')}
		<LineChart data={shaped} area={viz === 'area'} />
	{:else if shaped}
		<BarChart data={shaped} />
	{/if}
	{#if answer?.truncated}<p class="text-xs text-fg-subtle">
			Showing the first {answer.rows.length}.
		</p>{/if}
</section>
