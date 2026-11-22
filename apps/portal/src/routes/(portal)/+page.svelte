<script lang="ts">
	import { Ticket } from '@lucide/svelte';
	import { Card, EmptyState } from '@usehuxley/ui';
</script>

<svelte:head>
	<title>Home · Support portal</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">How can we help?</h1>
		<p class="mt-1 text-fg-muted">Track your requests and find answers in one place.</p>
	</div>

	<Card>
		<EmptyState
			title="Your tickets will appear here"
			description="The client portal opens in Phase 7, with tickets, service requests, devices, and help articles."
		>
			{#snippet icon()}<Ticket size={24} />{/snippet}
		</EmptyState>
	</Card>
</div>
