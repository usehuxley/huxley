<script lang="ts">
	import { enhance } from '$app/forms';
	import type { components } from '@usehuxley/api-client';
	import { Badge, Button, Card, Field, Input, type BadgeTone } from '@usehuxley/ui';

	type Change = components['schemas']['ScopeChange'];
	let { changes, canManage }: { changes: Change[]; canManage: boolean } = $props();
	const STATE: Record<string, [string, BadgeTone]> = {
		draft: ['Draft', 'neutral'],
		submitted: ['Waiting for the client', 'warning'],
		approved: ['Approved', 'success'],
		rejected: ['Rejected', 'danger'],
		withdrawn: ['Withdrawn', 'neutral']
	};
	const signed = (h: number) => (h > 0 ? `+${h}` : `${h}`);
</script>

<Card
	title="Scope changes"
	description="Work added or removed, approved by the client in the portal."
>
	<div class="flex flex-col gap-3">
		<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Scope changes">
			{#each changes as c (c.id)}
				<li class="flex flex-wrap items-center justify-between gap-2 py-2">
					<div class="min-w-0">
						<p class="font-medium text-fg">#{c.number} {c.title}</p>
						<p class="text-fg-muted">
							{signed(c.hours)} hours{c.amount !== null && c.amount !== undefined
								? ` · ${c.amount}`
								: ''}{c.decidedBy ? ` · by ${c.decidedBy}` : ''}{c.comment
								? `: “${c.comment}”`
								: ''}
						</p>
					</div>
					<span class="flex items-center gap-2">
						<Badge tone={STATE[c.state]?.[1] ?? 'neutral'}>{STATE[c.state]?.[0] ?? c.state}</Badge>
						{#if canManage && c.state === 'draft'}
							<form method="POST" action="?/submitScope" use:enhance>
								<input type="hidden" name="change" value={c.id} />
								<Button type="submit" size="sm" aria-label="Submit scope change {c.number}">
									Submit
								</Button>
							</form>
						{/if}
						{#if canManage && (c.state === 'draft' || c.state === 'submitted')}
							<form method="POST" action="?/withdrawScope" use:enhance>
								<input type="hidden" name="change" value={c.id} />
								<Button
									type="submit"
									size="sm"
									variant="ghost"
									aria-label="Withdraw scope change {c.number}"
								>
									Withdraw
								</Button>
							</form>
						{/if}
					</span>
				</li>
			{:else}
				<li class="py-2 text-fg-muted">None.</li>
			{/each}
		</ul>
		{#if canManage}
			<form
				method="POST"
				action="?/scope"
				class="grid gap-2 border-t border-border pt-3 sm:grid-cols-[1fr_7rem_8rem] sm:items-end"
				use:enhance
			>
				<Field label="Change" for="sc-title"><Input id="sc-title" name="title" required /></Field>
				<Field label="Hours (+/−)" for="sc-hours">
					<Input id="sc-hours" name="hours" type="number" step="0.5" required />
				</Field>
				<Field label="Price" for="sc-amount">
					<Input id="sc-amount" name="amount" type="number" step="0.01" />
				</Field>
				<div class="sm:col-span-3">
					<Field label="Why" for="sc-desc"><Input id="sc-desc" name="description" /></Field>
				</div>
				<div><Button type="submit">Draft scope change</Button></div>
			</form>
		{/if}
	</div>
</Card>
