<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { FileCode } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('script:manage'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	const languages = [
		{ value: 'pwsh', label: 'PowerShell 7' },
		{ value: 'python', label: 'Python 3' },
		{ value: 'javascript', label: 'JavaScript (Node.js)' }
	];
	const languageLabel = (l: string) =>
		l === 'pwsh' ? 'PowerShell' : l === 'javascript' ? 'JavaScript' : 'Python';
</script>

<svelte:head>
	<title>Scripts · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Scripts</h1>
		<p class="mt-1 text-sm text-fg-muted">
			PowerShell, Python and JavaScript scripts workflows run in sandboxed runners. Every change is
			kept as a version; organizations below share this library.
		</p>
	</header>
	<Card title="Library">
		{#if data.scripts.length === 0}
			<EmptyState title="No scripts yet" description="Add one below.">
				{#snippet icon()}<FileCode size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Scripts">
				{#each data.scripts as script (script.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/scripts/[id]', { id: script.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{script.name}
							</a>
							<p class="font-mono text-xs text-fg-muted">{script.key}</p>
						</div>
						<div class="flex items-center gap-2">
							<Badge tone="neutral">{languageLabel(script.language)}</Badge>
							<Badge tone="info">v{script.currentVersion}</Badge>
							{#if script.archived}<Badge tone="neutral">Archived</Badge>{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
	{#if canManage}
		<Card title="New script">
			<form method="POST" action="?/create" use:enhance class="flex flex-wrap items-end gap-3">
				<Field label="Name" for="name">
					<Input id="name" name="name" required maxlength={200} />
				</Field>
				<Field label="Language" for="language">
					<Select id="language" name="language" options={languages} value="pwsh" />
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
