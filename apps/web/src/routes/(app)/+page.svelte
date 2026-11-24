<script lang="ts">
	import { Badge, Button, Card } from '@usehuxley/ui';
	import { statusBadge } from '#lib/status.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let badge = $derived(statusBadge(data.status));

	const DOCS = 'https://docs.usehuxley.io';
	const START = [
		{
			title: 'Build a workflow',
			description: 'Start from a template, or from scratch in the visual editor or as YAML.',
			href: '/workflows',
			action: 'New workflow'
		},
		{
			title: 'Connect your tools',
			description: 'Import an API, or add Microsoft 365, Slack and others from the catalog.',
			href: '/integrations',
			action: 'Add an integration'
		},
		{
			title: 'Let AI help',
			description:
				'Describe what you need to the assistant, or connect an AI client like Claude Code.',
			href: '/assistant',
			action: 'Ask AI'
		},
		{
			title: 'Learn the basics',
			description:
				'A ten-minute tutorial: your first workflow, from draft to a form that starts it.',
			href: `${DOCS}/getting-started`,
			action: 'Read the tutorial'
		}
	];
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

	<Card title="Get started" description="Where to go first.">
		<ul class="flex flex-col divide-y divide-border text-sm" aria-label="Get started">
			{#each START as item (item.href)}
				<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="font-medium text-fg">{item.title}</p>
						<p class="mt-0.5 text-fg-muted">{item.description}</p>
					</div>
					<Button href={item.href} size="sm">{item.action}</Button>
				</li>
			{/each}
		</ul>
	</Card>
</div>
