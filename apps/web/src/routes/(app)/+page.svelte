<script lang="ts">
	import { Workflow } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState } from '@usehuxley/ui';
	import { statusBadge } from '#lib/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let badge = $derived(statusBadge(data.status));
</script>

<svelte:head>
	<title>Home · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Welcome to Huxley</h1>
		<p class="mt-1 text-fg-muted">
			The open-source automation platform for MSPs, MSSPs, and service desks.
		</p>
	</div>

	<Card title="Platform status" description="Health of the Huxley API and its dependencies.">
		{#snippet actions()}
			<Badge tone={badge.tone}>{badge.label}</Badge>
		{/snippet}
		<p class="text-sm text-fg-muted">
			{#if data.status.state === 'ready'}
				API version {data.status.version} is ready.
			{:else if data.status.state === 'degraded'}
				{data.status.detail}
			{:else}
				The web app could not reach the API. Check that the server is running and
				<code class="font-mono text-fg">HUXLEY_API_URL</code> points at it.
			{/if}
		</p>
	</Card>

	<Card>
		<EmptyState
			title="Workflows are coming"
			description="The automation engine and canvas arrive in Phase 2. Until then, this is the foundation: the app shell, design system, and API."
		>
			{#snippet icon()}<Workflow size={24} />{/snippet}
			{#snippet actions()}
				<Button href="https://docs.usehuxley.io" variant="primary">Read the docs</Button>
			{/snippet}
		</EmptyState>
	</Card>
</div>
