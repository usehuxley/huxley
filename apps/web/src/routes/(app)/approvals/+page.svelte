<script lang="ts">
	import { EmptyState } from '@usehuxley/ui';
	import { CircleCheck } from '@lucide/svelte';
	import ApprovalCard from '#lib/approvals/ApprovalCard.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let pending = $derived(data.approvals.filter((a) => a.status === 'pending'));
	let decided = $derived(data.approvals.filter((a) => a.status !== 'pending'));
</script>

<svelte:head>
	<title>Approvals · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Approvals</h1>
		<p class="mt-1 text-sm text-fg-muted">Decisions workflows are waiting for you to make.</p>
	</header>
	{#if form?.message}<p class="text-sm text-fg" role="status">{form.message}</p>{/if}
	{#if pending.length === 0}
		<EmptyState
			title="Nothing waiting for you"
			description="Approvals you're asked for show up here."
		>
			{#snippet icon()}<CircleCheck size={20} />{/snippet}
		</EmptyState>
	{/if}
	<section class="flex flex-col gap-4" aria-label="Waiting for you">
		{#each pending as approval (approval.id)}
			<ApprovalCard {approval} />
		{/each}
	</section>
	{#if decided.length}
		<section class="flex flex-col gap-4" aria-label="Decided recently">
			<h2 class="text-lg font-semibold text-fg">Decided recently</h2>
			{#each decided as approval (approval.id)}
				<ApprovalCard {approval} />
			{/each}
		</section>
	{/if}
</div>
