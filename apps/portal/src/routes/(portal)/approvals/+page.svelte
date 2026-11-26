<script lang="ts">
	import { resolve } from '$app/paths';
	import { CircleCheck } from '@lucide/svelte';
	import { Badge, Card, EmptyState } from '@usehuxley/ui';
	import { APPROVAL_STATUS } from '#lib/approvals.ts';
	import { when } from '#lib/tickets/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Approvals · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Approvals</h1>
		<p class="mt-1 text-fg-muted">Requests that need a yes or no from you.</p>
	</div>
	<Card>
		{#if data.approvals.length === 0}
			<EmptyState
				title="Nothing to approve"
				description="We’ll email you when something needs you."
			>
				{#snippet icon()}<CircleCheck size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Approvals">
				{#each data.approvals as a (a.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(portal)/approvals/[id]', { id: a.id })}
								class="font-medium text-fg hover:underline"
							>
								{a.title}
							</a>
							<p class="text-xs text-fg-muted">
								Asked {when(a.createdAt)}{a.decidedByName ? ` · by ${a.decidedByName}` : ''}
							</p>
						</div>
						<Badge tone={APPROVAL_STATUS[a.status]?.tone ?? 'neutral'}>
							{APPROVAL_STATUS[a.status]?.label ?? a.status}
						</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
