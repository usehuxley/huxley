<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import { statusBadge } from '#lib/status.ts';
	import SlaBar from '#lib/tickets/SlaBar.svelte';
	import RunStatusBadge from '#lib/workflows/RunStatusBadge.svelte';
	import { ATTENTION } from '#lib/tickets/attention.ts';
	import { priorityOf } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let badge = $derived(statusBadge(data.status));
	let firstName = $derived(data.user.displayName.split(/\s+/)[0] ?? data.user.displayName);

	// The greeting and the date follow the browser's clock, not the server's.
	let now = $state(Date.now());
	$effect(() => {
		now = Date.now();
		const timer = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(timer);
	});
	let greeting = $derived.by(() => {
		const hour = new Date(now).getHours();
		return hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
	});
	let today = $derived(
		new Date(now).toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })
	);

	let mine = $derived(data.work?.mine ?? []);
	let attention = $derived(data.work?.attention ?? {});
	let needing = $derived(mine.filter((t) => attention[t.id] && attention[t.id] !== 'new'));
	let unassigned = $derived(
		(data.work?.queues ?? []).reduce((n, q) => n + q.unassigned, 0) + (data.work?.team.length ?? 0)
	);

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

<div class="mx-auto flex max-w-6xl flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div class="min-w-0">
			<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">
				{greeting}, {firstName}
			</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				{today}{#if data.work}
					· {needing.length === 0
						? 'nothing urgent'
						: `${needing.length} ticket${needing.length === 1 ? ' needs' : 's need'} you`}{/if}
			</p>
		</div>
		{#if data.work}
			<div class="ml-auto flex gap-2">
				<Button href={resolve('/(app)/tickets')} variant="primary">Open the desk</Button>
			</div>
		{/if}
	</div>

	{#if data.work}
		<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
			{#each [{ label: 'Need attention', value: needing.length, hot: needing.length > 0, note: 'missed SLAs, replies, due soon' }, { label: 'Assigned to you', value: mine.length, hot: false, note: 'open tickets' }, { label: 'Unassigned', value: unassigned, hot: false, note: 'in your queues and teams' }, { label: 'Waiting on your approval', value: data.approvals.length, hot: false, note: 'workflow approvals' }] as stat (stat.label)}
				<div class="rounded-card border border-border bg-surface px-4 py-3">
					<p class="text-[12.5px] font-semibold text-fg-subtle">{stat.label}</p>
					<p
						class="text-[28px] leading-tight font-extrabold tracking-tight tabular-nums {stat.hot
							? 'text-danger'
							: 'text-fg'}"
					>
						{stat.value}
					</p>
					<p class="text-[12.5px] text-fg-subtle">{stat.note}</p>
				</div>
			{/each}
		</div>
	{/if}

	<div class="grid items-start gap-4 lg:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
		<div class="flex min-w-0 flex-col gap-4">
			{#if data.work}
				<Card title="Needs you now">
					{#snippet actions()}
						<a
							href={resolve('/(app)/my-work')}
							class="text-sm font-semibold text-accent hover:underline"
						>
							All my work
						</a>
					{/snippet}
					{#if mine.length === 0}
						<p class="text-sm text-fg-muted">Nothing is assigned to you. Enjoy it.</p>
					{:else}
						<ul class="-mx-5 -my-4 divide-y divide-border" aria-label="Needs you now">
							{#each mine.slice(0, 8) as t (t.id)}
								{@const priority = priorityOf(t.priority)}
								{@const why = attention[t.id] ? ATTENTION[attention[t.id]] : undefined}
								<li>
									<a
										href={resolve('/(app)/tickets/[id]', { id: t.id })}
										class="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 px-5 py-2.5 hover:bg-surface-sunken"
									>
										<Badge tone={priority.tone}>{priority.short}</Badge>
										<span class="min-w-0">
											<span class="block truncate text-sm font-semibold text-fg">{t.title}</span>
											<span class="block truncate text-xs text-fg-subtle">
												{t.reference} · {t.orgName}{why ? ` · ${why.label}` : ''}
											</span>
										</span>
										<SlaBar
											createdAt={t.createdAt}
											dueAt={t.resolutionMet == null ? t.resolutionDueAt : null}
											paused={t.slaPaused}
											{now}
										/>
									</a>
								</li>
							{/each}
						</ul>
					{/if}
				</Card>
			{/if}

			{#if data.approvals.length > 0}
				<Card title="Waiting on your approval">
					<ul class="-mx-5 -my-4 divide-y divide-border" aria-label="Waiting on your approval">
						{#each data.approvals as a (a.id)}
							<li>
								<a
									href={resolve('/(app)/approvals/[id]', { id: a.id })}
									class="flex items-center gap-3 px-5 py-2.5 hover:bg-surface-sunken"
								>
									<Badge tone="warning">Approval</Badge>
									<span class="min-w-0 flex-1">
										<span class="block truncate text-sm font-semibold text-fg">{a.title}</span>
										<span class="block truncate text-xs text-fg-subtle">
											{a.workflowName ?? 'Workflow'} · expires {new Date(
												a.expiresAt
											).toLocaleString()}
										</span>
									</span>
								</a>
							</li>
						{/each}
					</ul>
				</Card>
			{/if}

			{#if !data.work}
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
			{/if}
		</div>

		<div class="flex min-w-0 flex-col gap-4">
			<Card title="Runs you started">
				{#if data.runs.length === 0}
					<p class="text-sm text-fg-muted">
						None lately. Actions you run, and workflows you start, show here.
					</p>
				{:else}
					<ul class="-mx-5 -my-4 divide-y divide-border" aria-label="Runs you started">
						{#each data.runs as run (run.id)}
							<li>
								<a
									href={resolve('/(app)/runs/[id]', { id: run.id })}
									class="flex items-center gap-3 px-5 py-2.5 hover:bg-surface-sunken"
								>
									<span class="min-w-0 flex-1">
										<span class="block truncate text-sm font-semibold text-fg">
											{run.workflowName}
										</span>
										<span class="block text-xs text-fg-subtle">
											{new Date(run.createdAt).toLocaleString()}
										</span>
									</span>
									<RunStatusBadge status={run.status} />
								</a>
							</li>
						{/each}
					</ul>
				{/if}
			</Card>

			<Card title="Platform status">
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
		</div>
	</div>
</div>
