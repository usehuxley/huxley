<!-- A ticket's AI summary, the things it mentions, a next step, and tickets like it. -->
<script lang="ts">
	import AiLabel from '#lib/ai/AiLabel.svelte';
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { RefreshCw } from '@lucide/svelte';
	import { Badge, Button, Card, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let {
		insight,
		similar
	}: {
		insight: components['schemas']['TicketInsight'] | null;
		similar: components['schemas']['SimilarTickets'] | null;
	} = $props();

	const KINDS = [
		['people', 'People'],
		['devices', 'Devices'],
		['applications', 'Applications'],
		['errors', 'Errors']
	] as const;
	let entities = $derived((insight?.entities ?? {}) as Record<string, string[] | undefined>);
</script>

<Card title="Insight">
	{#if insight}
		<p class="text-sm text-fg" aria-label="Summary">{insight.summary}</p>
		{#if insight.nextStep}
			<p class="mt-2 text-sm text-fg"><strong>Next step:</strong> {insight.nextStep}</p>
		{/if}
		<dl class="mt-2 flex flex-col gap-1 text-xs">
			{#each KINDS as [key, label] (key)}
				{#if (entities[key] ?? []).length > 0}
					<div class="flex flex-wrap items-center gap-1">
						<dt class="text-fg-muted">{label}:</dt>
						{#each entities[key] ?? [] as name (name)}<dd><Badge>{name}</Badge></dd>{/each}
					</div>
				{/if}
			{/each}
		</dl>
		<p class="mt-2 flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
			<AiLabel />
			Summarized by AI, {new Date(insight.at).toLocaleString()}
		</p>
	{:else}
		<p class="text-sm text-fg-muted">
			No summary yet. With the AI ticket insights pack, it's written as the conversation grows.
		</p>
	{/if}
	<form
		method="POST"
		action="?/insight"
		class="mt-2"
		use:enhance={() =>
			async ({ result, update }) => {
				await update({ reset: false });
				if (result.type === 'success') {
					toast.push({
						title: 'Asked for a fresh summary: it shows here shortly.',
						tone: 'success'
					});
				}
			}}
	>
		<Button type="submit" variant="ghost"><RefreshCw size={14} /> Refresh</Button>
	</form>
	{#if similar?.available && similar.tickets.length > 0}
		<h3 class="mt-3 text-sm font-medium text-fg">Similar tickets</h3>
		<ul class="mt-1 flex flex-col gap-1 text-sm" aria-label="Similar tickets">
			{#each similar.tickets as t (t.id)}
				<li class="flex items-center justify-between gap-2">
					<a
						href={resolve('/(app)/tickets/[id]', { id: t.id })}
						class="truncate text-accent hover:underline"
					>
						{t.reference} · {t.title}
					</a>
					<span class="shrink-0 text-xs text-fg-muted">
						{t.statusName} · {Math.round(t.score * 100)}%
					</span>
				</li>
			{/each}
		</ul>
	{/if}
</Card>
