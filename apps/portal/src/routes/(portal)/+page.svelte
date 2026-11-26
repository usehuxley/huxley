<script lang="ts">
	import { Ticket } from '@lucide/svelte';
	import { Card, EmptyState } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let firstName = $derived(data.me.name.split(' ')[0] || data.me.name);
</script>

<svelte:head>
	<title>Home · Support portal</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">How can we help, {firstName}?</h1>
		<p class="mt-1 text-fg-muted">Support for {data.me.orgName}, in one place.</p>
	</div>

	<Card>
		<EmptyState
			title="Your tickets will appear here"
			description="Seeing and raising tickets in the portal is on its way."
		>
			{#snippet icon()}<Ticket size={24} />{/snippet}
		</EmptyState>
	</Card>
</div>
