<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field } from '@usehuxley/ui';
	import { Widget } from '@usehuxley/widgets';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let s = $derived(data.submission);
	let names = (ids: string[], from: { id: string; name: string }[]) =>
		ids.map((id) => from.find((o) => o.id === id)?.name ?? 'Unknown');
	let teams = $derived(names(s.teams, data.audiences.teams));
	let roles = $derived(names(s.roles, data.audiences.roles));
	const STATUS = {
		pending: ['Waiting for approval', 'warning'],
		approved: ['Published', 'success'],
		rejected: ['Sent back', 'danger'],
		withdrawn: ['Withdrawn', 'neutral']
	} as const;
</script>

<svelte:head><title>Review {s.snapshot.name} · Huxley</title></svelte:head>

<div class="flex flex-col gap-4">
	<div>
		<a href={resolve('/(app)/dashboards')} class="text-sm text-accent hover:underline">
			Dashboards
		</a>
		<h1 class="mt-1 flex flex-wrap items-center gap-2 text-2xl font-semibold text-fg">
			Review “{s.snapshot.name}”
			<Badge tone={STATUS[s.status][1]}>{STATUS[s.status][0]}</Badge>
		</h1>
		{#if s.snapshot.description}<p class="text-fg-muted">{s.snapshot.description}</p>{/if}
	</div>
	<Card title="The request">
		<dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[max-content_1fr]">
			<dt class="text-fg-muted">Submitted by</dt>
			<dd class="text-fg">
				{s.submittedByName ?? 'Someone'} on {new Date(s.submittedAt).toLocaleDateString()}
			</dd>
			<dt class="text-fg-muted">Share with</dt>
			<dd class="text-fg">
				{[...teams, ...roles].join(', ') || 'Everyone who sees dashboards'}
			</dd>
			<dt class="text-fg-muted">Client portal</dt>
			<dd class="text-fg">{s.portal ? 'Yes: client admins see their company’s numbers' : 'No'}</dd>
			{#if s.note}
				<dt class="text-fg-muted">Note</dt>
				<dd class="whitespace-pre-line text-fg">{s.note}</dd>
			{/if}
			{#if s.comment}
				<dt class="text-fg-muted">Reviewer’s comment</dt>
				<dd class="text-fg">{s.comment}</dd>
			{/if}
		</dl>
		{#if data.canDecide}
			<div class="mt-4 flex flex-col gap-3 border-t border-border pt-4">
				<form method="POST" action="?/reject" class="flex flex-col gap-2" use:enhance>
					<Field label="Comment (required to send it back)" for="review-comment">
						<textarea
							id="review-comment"
							name="comment"
							rows="2"
							maxlength="1000"
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
						></textarea>
					</Field>
					<div class="flex flex-wrap gap-2">
						<Button type="submit" formaction="?/approve" variant="primary">
							Approve and publish
						</Button>
						<Button type="submit">Send back</Button>
					</div>
				</form>
				{#if form && 'message' in form}<p class="text-sm text-danger" role="alert">
						{form.message}
					</p>{/if}
			</div>
		{/if}
	</Card>
	<p class="text-sm text-fg-muted">As submitted, with the numbers you can see:</p>
	<div class="grid grid-cols-1 gap-4 md:grid-cols-12">
		{#each s.snapshot.widgets as w (w.id)}
			<Widget
				title={w.title}
				viz={w.viz}
				width={w.width}
				answer={data.results[w.id]?.answer}
				problem={data.results[w.id]?.problem}
			/>
		{/each}
	</div>
</div>
