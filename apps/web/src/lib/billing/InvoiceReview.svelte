<!-- An invoice's internal review (ADR-076): why it needs one, what changed
     since the last, the decisions so far, and (for its reviewers) the
     decision. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { ShieldCheck } from '@lucide/svelte';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { REVIEW_STATUSES, label, reviewTone } from '#lib/sales/labels.ts';

	let {
		review,
		approval,
		canSubmit,
		version,
		dirty,
		done
	}: {
		review: components['schemas']['InvoiceReviewView'];
		approval: components['schemas']['ApprovalView'] | null;
		canSubmit: boolean;
		version: number;
		dirty: boolean;
		done: (message: string) => SubmitFunction;
	} = $props();

	let submittable = $derived(
		canSubmit && review.reasons.length > 0 && review.status !== 'in_review' && !review.canApprove
	);
	const decisionLabel = (d: string | null | undefined) =>
		d ? label(REVIEW_STATUSES, d) : 'Waiting for a decision';
</script>

<Card title="Review">
	<div class="flex flex-col gap-3 text-sm">
		<p class="flex flex-wrap items-center gap-2">
			<ShieldCheck size={15} class="text-fg-subtle" aria-hidden="true" />
			<Badge tone={reviewTone(review.status)}>{label(REVIEW_STATUSES, review.status)}</Badge>
			{#if review.overridden}<Badge tone="warning">Approved without review</Badge>{/if}
		</p>
		{#if review.reasons.length > 0}
			<ul class="flex flex-col gap-1" aria-label="Why it needs review">
				{#each review.reasons as r (r.ruleId)}
					<li>
						<span class="font-semibold text-fg">{r.name}</span>
						<span class="block text-xs text-fg-muted">{r.detail}</span>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="text-fg-muted">No review rule matches this invoice.</p>
		{/if}

		{#if review.changes}
			<div class="rounded-control bg-surface-sunken px-3 py-2 text-xs" aria-label="Changed since">
				<p class="mb-1 font-semibold text-fg">
					Since revision {review.changes.sinceRevision} was {label(
						REVIEW_STATUSES,
						review.changes.decision
					).toLowerCase()}
				</p>
				<ul class="flex flex-col gap-0.5 text-fg-muted">
					{#each review.changes.added as a, i (i)}<li>Added {a}</li>{/each}
					{#each review.changes.removed as r, i (i)}<li>Removed {r}</li>{/each}
					{#each review.changes.changed as c, i (i)}
						<li>
							{c.name}: {c.fields.map((f) => `${f.field} ${f.before} → ${f.after}`).join(', ')}
						</li>
					{/each}
					{#if review.changes.total}
						<li>Total {review.changes.total.before} → {review.changes.total.after}</li>
					{/if}
				</ul>
			</div>
		{/if}

		{#if approval}
			<form
				method="POST"
				action="?/decideReview"
				use:enhance={done('Review recorded.')}
				class="flex flex-col gap-2 border-t border-border pt-3"
			>
				<p class="font-semibold text-fg">Your review</p>
				<input type="hidden" name="id" value={approval.id} />
				<textarea
					name="comment"
					rows="2"
					maxlength="2000"
					aria-label="Review comment"
					placeholder="What should change, or why (optional)"
					class="rounded-control border border-border bg-surface p-2 text-sm text-fg"></textarea>
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" name="decision" value="approve" variant="primary">
						Approve
					</Button>
					{#if approval.allowChanges}
						<Button type="submit" size="sm" name="decision" value="request_changes">
							Request changes
						</Button>
					{/if}
					<Button type="submit" size="sm" name="decision" value="reject" variant="danger">
						Reject
					</Button>
				</div>
			</form>
		{/if}

		{#if submittable}
			<form method="POST" action="?/submitReview" use:enhance={done('Submitted for review.')}>
				<input type="hidden" name="version" value={version} />
				<Button
					type="submit"
					size="sm"
					variant="primary"
					disabled={dirty}
					title={dirty ? 'Save first' : undefined}
				>
					Submit for review
				</Button>
			</form>
		{/if}

		{#if review.history.length > 0}
			<ol class="flex flex-col gap-2 border-t border-border pt-3" aria-label="Review history">
				{#each review.history as h (h.id)}
					<li>
						<span class="font-semibold text-fg">
							Revision {h.revision}: {decisionLabel(h.decision)}
						</span>
						<span class="block text-xs text-fg-subtle">
							{h.decidedBy ? `By ${h.decidedBy}` : `Submitted by ${h.submittedByName ?? 'someone'}`}
							· {new Date(h.decidedAt ?? h.submittedAt).toLocaleString()}
						</span>
						{#if h.comment}<span class="block text-xs text-fg-muted">“{h.comment}”</span>{/if}
					</li>
				{/each}
			</ol>
		{/if}
	</div>
</Card>
