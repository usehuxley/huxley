<!-- One approval: what's asked, and (while pending) approve or reject. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let {
		approval,
		action = '?/decide'
	}: { approval: components['schemas']['ApprovalView']; action?: string } = $props();

	let pending = $derived(approval.status === 'pending');
	const tone = (status: string) =>
		status === 'approved'
			? 'success'
			: status === 'rejected'
				? 'danger'
				: status === 'pending'
					? 'warning'
					: 'neutral';
	const when = (value: string) => new Date(value).toLocaleString();
</script>

<Card title={approval.title}>
	<div class="flex flex-col gap-3 text-sm">
		<p class="flex items-center gap-2">
			<Badge tone={tone(approval.status)}>{approval.status}</Badge>
			<span class="text-fg-muted">
				{approval.workflowName ?? 'Workflow'} ·
				<a
					href={resolve('/(app)/runs/[id]', { id: approval.runId })}
					class="text-accent hover:underline"
				>
					run
				</a>
				· {pending
					? `expires ${when(approval.expiresAt)}`
					: `decided ${when(approval.decidedAt ?? approval.createdAt)}`}
			</span>
		</p>
		{#if approval.message}<p class="whitespace-pre-line text-fg">{approval.message}</p>{/if}
		{#if approval.comment}<p class="text-fg-muted">“{approval.comment}”</p>{/if}
		{#if pending}
			<form method="POST" {action} use:enhance class="flex flex-col gap-2">
				<input type="hidden" name="id" value={approval.id} />
				<textarea
					name="comment"
					rows="2"
					maxlength="2000"
					aria-label="Comment (optional)"
					placeholder="Comment (optional)"
					class="rounded-control border border-border bg-surface p-2 text-sm text-fg"></textarea>
				<div class="flex gap-2">
					<Button type="submit" name="decision" value="approve">Approve</Button>
					<Button type="submit" name="decision" value="reject" variant="danger">Reject</Button>
				</div>
			</form>
		{/if}
	</div>
</Card>
