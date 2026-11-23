<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Workflow } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canWrite = $derived(data.user.grantedPermissions.includes('workflow:write'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.currentOrg?.id ?? data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
</script>

<svelte:head>
	<title>Workflows · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Workflows</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Automations that run on a schedule, from a webhook, or when someone starts them.
			<a href={resolve('/(app)/runs')} class="text-accent hover:underline">All runs</a>
		</p>
	</header>

	<Card title="All workflows">
		{#if data.workflows.length === 0}
			<EmptyState title="No workflows yet" description="Create one below to automate a task.">
				{#snippet icon()}<Workflow size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Workflows">
				{#each data.workflows as workflow (workflow.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/workflows/[id]', { id: workflow.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{workflow.name}
							</a>
							<p class="mt-0.5 font-mono text-xs text-fg-muted">
								{workflow.key} · {orgName.get(workflow.orgId) ?? 'Unknown organization'}
							</p>
						</div>
						<div class="flex items-center gap-2">
							{#if workflow.status === 'archived'}
								<Badge tone="neutral">Archived</Badge>
							{:else if workflow.currentVersion}
								<Badge tone="success">Version {workflow.currentVersion}</Badge>
							{:else}
								<Badge tone="warning">Draft</Badge>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if canWrite}
		<Card title="New workflow" description="Starts as a draft with one step to show the shape.">
			<form method="POST" action="?/create" class="flex flex-wrap items-end gap-3" use:enhance>
				<Field label="Name" for="name">
					<Input id="name" name="name" required maxlength={200} />
				</Field>
				<Field label="Key" for="key" hint="Optional; made from the name">
					<Input id="key" name="key" pattern="[a-z][a-z0-9-]*" maxlength={64} />
				</Field>
				<Field label="Organization" for="orgId">
					<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
				</Field>
				<Button type="submit">Create</Button>
			</form>
			{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		</Card>
	{/if}
</div>
