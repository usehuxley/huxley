<script lang="ts">
	import { Badge, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { survey }: { survey: components['schemas']['CsatSurvey'] } = $props();

	let positive = $derived(
		survey.rating === null || survey.rating === undefined
			? null
			: survey.scale === 'thumbs'
				? survey.rating === 5
				: survey.rating >= 4
	);
	let rating = $derived(
		survey.rating === null || survey.rating === undefined
			? null
			: survey.scale === 'thumbs'
				? survey.rating === 5
					? 'Thumbs up'
					: 'Thumbs down'
				: `${survey.rating} out of 5`
	);
	const when = (t: string) => new Date(t).toLocaleString();
</script>

<Card title="Satisfaction">
	{#if rating}
		<p class="flex items-center gap-2">
			<Badge tone={positive ? 'success' : 'warning'}>{rating}</Badge>
			<span class="text-xs text-fg-muted">
				{survey.channel === 'portal' ? 'In the portal' : 'From the email'} ·
				{when(survey.respondedAt!)}
			</span>
		</p>
		{#if survey.comment}<p class="mt-2 text-sm whitespace-pre-line text-fg">
				“{survey.comment}”
			</p>{/if}
	{:else}
		<p class="text-sm text-fg-muted">
			Asked {when(survey.sentAt)}{survey.emailed
				? ' by email and in the portal'
				: ' in the portal'};
			{new Date(survey.expiresAt) > new Date() ? 'no answer yet.' : 'it wasn’t answered.'}
		</p>
	{/if}
</Card>
