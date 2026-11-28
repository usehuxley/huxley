<!-- How the client's messages on this ticket sounded (the AI sentiment pack). -->
<script lang="ts">
	import AiLabel from '#lib/ai/AiLabel.svelte';
	import { Badge, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { entries }: { entries: components['schemas']['SentimentEntry'][] } = $props();

	const LABEL: Record<number, [string, 'danger' | 'warning' | 'neutral' | 'success']> = {
		[-2]: ['Very negative', 'danger'],
		[-1]: ['Negative', 'warning'],
		0: ['Neutral', 'neutral'],
		1: ['Positive', 'success'],
		2: ['Very positive', 'success']
	};
	let latest = $derived(entries[entries.length - 1]);
</script>

<Card title="Sentiment">
	<p class="flex flex-wrap items-center gap-2">
		<Badge tone={LABEL[latest.score][1]}>{LABEL[latest.score][0]}</Badge>
		<span class="text-sm text-fg">{latest.reason}</span>
	</p>
	{#if entries.length > 1}
		<ul class="mt-2 flex flex-col gap-1 text-xs text-fg-muted" aria-label="Earlier messages">
			{#each entries.slice(0, -1).reverse() as e (e.id)}
				<li>{LABEL[e.score][0]}: {e.reason}</li>
			{/each}
		</ul>
	{/if}
	<p class="mt-2 flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
		<AiLabel />
		Judged by AI from the client's messages.
	</p>
</Card>
