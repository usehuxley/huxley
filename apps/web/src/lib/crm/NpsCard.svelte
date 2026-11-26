<script lang="ts">
	import { Badge, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { nps }: { nps: components['schemas']['NpsSurvey'] } = $props();
	const CATEGORY = (score: number) =>
		score >= 9
			? { label: 'Promoter', tone: 'success' as const }
			: score >= 7
				? { label: 'Passive', tone: 'neutral' as const }
				: { label: 'Detractor', tone: 'warning' as const };
	const when = (t: string) => new Date(t).toLocaleDateString();
</script>

<Card title="Net promoter">
	{#if nps.score !== null && nps.score !== undefined}
		{@const c = CATEGORY(nps.score)}
		<p class="flex items-center gap-2">
			<span class="text-2xl font-semibold text-fg tabular-nums">{nps.score}</span>
			<span class="text-sm text-fg-muted">out of 10</span>
			<Badge tone={c.tone}>{c.label}</Badge>
		</p>
		<p class="mt-1 text-xs text-fg-muted">
			{nps.campaign} · {nps.channel === 'portal' ? 'in the portal' : 'from the email'} ·
			{when(nps.respondedAt!)}
		</p>
		{#if nps.comment}<p class="mt-2 text-sm whitespace-pre-line text-fg">“{nps.comment}”</p>{/if}
	{:else}
		<p class="text-sm text-fg-muted">
			Asked {when(nps.sentAt)} ({nps.campaign});
			{new Date(nps.expiresAt) > new Date() ? 'no answer yet.' : 'it wasn’t answered.'}
		</p>
	{/if}
</Card>
