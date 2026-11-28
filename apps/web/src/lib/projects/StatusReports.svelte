<script lang="ts">
	import AiLabel from '#lib/ai/AiLabel.svelte';
	import { enhance } from '$app/forms';
	import type { components } from '@usehuxley/api-client';
	import { Badge, Button, Card, ReportBody } from '@usehuxley/ui';
	import { HEALTH } from './labels.ts';

	type Report = components['schemas']['StatusReport'];
	let { reports, canManage }: { reports: Report[]; canManage: boolean } = $props();
	const when = (r: Report) =>
		r.periodStart && r.periodEnd
			? `${r.periodStart} to ${r.periodEnd}`
			: new Date(r.createdAt).toLocaleDateString();
</script>

<Card
	title="Status reports"
	description="Written here, or drafted weekly by the AI project status reports pack. Share one to show it in the client’s portal."
>
	<div class="flex flex-col gap-4">
		{#each reports as r (r.id)}
			<article
				class="flex flex-col gap-2 border-b border-border pb-3 last:border-0"
				aria-label="Status report, {when(r)}"
			>
				<header class="flex flex-wrap items-center justify-between gap-2 text-sm">
					<span class="flex flex-wrap items-center gap-2 text-fg-muted">
						{when(r)} · {r.author}
						{#if r.automated}<AiLabel text="Drafted automatically" />{/if}
					</span>
					<span class="flex items-center gap-2">
						<Badge tone={HEALTH[r.health]?.[1] ?? 'neutral'}>
							{HEALTH[r.health]?.[0] ?? r.health}
						</Badge>
						{#if r.clientVisible}<Badge tone="success">Shared with the client</Badge>{/if}
						{#if canManage}
							<form method="POST" action="?/shareReport" use:enhance>
								<input type="hidden" name="report" value={r.id} />
								<input type="hidden" name="visible" value={r.clientVisible ? 'false' : 'true'} />
								<Button type="submit" size="sm" variant="ghost">
									{r.clientVisible ? 'Stop sharing' : 'Share with the client'}
								</Button>
							</form>
							<form method="POST" action="?/deleteReport" use:enhance>
								<input type="hidden" name="report" value={r.id} />
								<Button type="submit" size="sm" variant="ghost">Delete</Button>
							</form>
						{/if}
					</span>
				</header>
				<ReportBody body={r.body} />
			</article>
		{:else}
			<p class="text-sm text-fg-muted">None yet.</p>
		{/each}
		{#if canManage}
			<form
				method="POST"
				action="?/report"
				class="flex flex-col gap-2 border-t border-border pt-3"
				use:enhance
			>
				<label for="sr-body" class="text-sm font-medium text-fg">Write a report</label>
				<textarea
					id="sr-body"
					name="body"
					rows="4"
					required
					placeholder="What got done, what's next, and anything in the way. A line in **bold** is a heading; start lines with - for a list."
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				></textarea>
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="clientVisible" /> Share with the client
				</label>
				<div><Button type="submit">Save report</Button></div>
			</form>
		{/if}
	</div>
</Card>
