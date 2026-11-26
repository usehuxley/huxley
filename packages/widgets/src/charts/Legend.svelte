<script lang="ts">
	import { color, type Series } from '../data';

	let { series }: { series: Series[] } = $props();
</script>

{#if series.length > 1}
	<ul class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-muted" aria-label="Legend">
		{#each series as s (s.key)}
			<li class="flex items-center gap-1.5">
				<span class="inline-block size-2.5 rounded-sm" style="background: {color(s.slot)}"></span>
				{s.label}
			</li>
		{/each}
	</ul>
{/if}
