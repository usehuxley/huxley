<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field } from '@usehuxley/ui';
	import { APPROVAL_STATUS } from '#lib/approvals.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let a = $derived(data.approval);
	let status = $derived(APPROVAL_STATUS[a.status] ?? { label: a.status, tone: 'neutral' as const });
</script>

<svelte:head>
	<title>{a.title} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<div>
		<a href={resolve('/(portal)/approvals')} class="text-sm text-accent hover:underline">
			Approvals
		</a>
		<h1 class="mt-1 text-2xl font-semibold text-fg">{a.title}</h1>
		<div class="mt-2"><Badge tone={status.tone}>{status.label}</Badge></div>
	</div>
	<Card>
		{#if a.message}<p class="text-sm whitespace-pre-line text-fg">{a.message}</p>{/if}
		{#if a.status === 'pending'}
			<p class="mt-2 text-xs text-fg-muted">
				Decide by {new Date(a.expiresAt).toLocaleString()}.
			</p>
			<form method="POST" class="mt-4 flex flex-col gap-3" use:enhance>
				<Field label="Comment (optional)" for="comment">
					<textarea
						id="comment"
						name="comment"
						rows="3"
						maxlength="2000"
						class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
					></textarea>
				</Field>
				<div class="flex gap-2">
					<Button type="submit" name="decision" value="approve" variant="primary">Approve</Button>
					{#if a.allowChanges}
						<Button type="submit" name="decision" value="request_changes">Ask for changes</Button>
					{/if}
					<Button type="submit" name="decision" value="reject" variant="danger">Reject</Button>
				</div>
			</form>
		{:else}
			<p class="mt-3 text-sm text-fg-muted">
				{#if a.decidedByName}{status.label} by {a.decidedByName}{#if a.decidedAt}
						on {new Date(a.decidedAt).toLocaleString()}{/if}.{:else}{status.label}.{/if}
			</p>
			{#if a.comment}<p class="mt-2 text-sm text-fg">“{a.comment}”</p>{/if}
		{/if}
		{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
	</Card>
</div>
