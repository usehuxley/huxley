<!-- The weights this organization's clients' health is scored with. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Card, Field, Input } from '@usehuxley/ui';

	let { weights, message }: { weights: Record<string, number>; message?: string | null } = $props();

	const FACTORS: [string, string][] = [
		['csat', 'Satisfaction surveys'],
		['nps', 'Net promoter score'],
		['sentiment', 'Client sentiment'],
		['sla', 'SLA targets met'],
		['reopens', 'Tickets staying resolved'],
		['volume', 'Ticket volume'],
		['renewal', 'Agreement renewals']
	];
</script>

<Card
	title="Client health model"
	description="How much each factor counts toward the health score of clients here and below: 0 leaves it out, 10 counts most."
>
	<form method="POST" action="?/healthModel" class="flex flex-col gap-4" use:enhance>
		<div class="grid gap-3 sm:grid-cols-3">
			{#each FACTORS as [key, label] (key)}
				<Field {label} for="w-{key}">
					<Input
						id="w-{key}"
						name={key}
						type="number"
						min="0"
						max="10"
						value={weights[key] ?? 0}
						class="w-24"
					/>
				</Field>
			{/each}
		</div>
		{#if message}<p class="text-sm text-fg-muted" role="status">{message}</p>{/if}
		<div><Button type="submit">Save weights</Button></div>
	</form>
</Card>
