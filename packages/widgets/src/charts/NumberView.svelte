<script lang="ts">
	import { formatValue, type Answer } from '../data';

	let { answer }: { answer: Answer } = $props();
	let column = $derived(answer.columns[answer.columns.length - 1]);
	let value = $derived(answer.rows[0]?.[answer.columns.length - 1] ?? null);
</script>

{#if value === null}
	<p class="text-3xl font-semibold text-fg-subtle">–</p>
	<p class="text-xs text-fg-muted">No data in this range</p>
{:else}
	<p class="text-3xl font-semibold text-fg tabular-nums">
		{formatValue(value, column?.kind ?? 'number')}
	</p>
{/if}
