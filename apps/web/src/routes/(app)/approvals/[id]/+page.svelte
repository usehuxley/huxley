<script lang="ts">
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import ApprovalCard from '#lib/approvals/ApprovalCard.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>{data.approval.title} · Approvals · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<a
		href={resolve('/(app)/approvals')}
		class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={14} /> Approvals
	</a>
	{#if form?.message}<p class="text-sm text-fg" role="status">{form.message}</p>{/if}
	<ApprovalCard approval={data.approval} />
</div>
