<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, CodeEditor, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Plug } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('integration:manage'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	let importing = $state(false);
</script>

<svelte:head>
	<title>Integrations · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Integrations</h1>
		<p class="mt-1 text-sm text-fg-muted">
			APIs imported from their OpenAPI or Swagger specs. Each operation becomes an action workflows
			can call.
		</p>
	</header>

	<Card title="All integrations">
		{#if data.integrations.length === 0}
			<EmptyState title="No integrations yet" description="Import an API's spec below.">
				{#snippet icon()}<Plug size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Integrations">
				{#each data.integrations as integration (integration.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/integrations/[id]', { id: integration.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{integration.name}
							</a>
							<p class="mt-0.5 font-mono text-xs text-fg-muted">
								{integration.key} · {orgName.get(integration.orgId) ?? 'Unknown organization'}
							</p>
						</div>
						<Badge tone="neutral">Version {integration.currentVersion}</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if canManage && data.catalog.length}
		<Card
			title="Built-in integrations"
			description="Curated for common MSP work. Added to the organization chosen below, then used like any other."
		>
			<ul class="flex flex-col divide-y divide-border" aria-label="Built-in integrations">
				{#each data.catalog as entry (entry.key)}
					{@const added = data.integrations.some((i) => i.key === entry.key && i.orgId === orgId)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0 text-sm">
							<p class="font-medium text-fg">
								{entry.name}
								<Badge tone="neutral">{entry.category}</Badge>
							</p>
							<p class="text-xs text-fg-muted">{entry.summary} {entry.actions} actions.</p>
						</div>
						{#if added}
							<span class="text-xs text-fg-muted">Added</span>
						{:else}
							<form method="POST" action="?/addBuiltIn" use:enhance>
								<input type="hidden" name="key" value={entry.key} />
								<input type="hidden" name="orgId" value={orgId} />
								<Button type="submit" size="sm">Add {entry.name}</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if canManage}
		<Card
			title="Import an API"
			description="OpenAPI 3.0 or 3.1, or Swagger 2.0, as JSON or YAML. Paste it or upload the file."
		>
			<form
				method="POST"
				action="?/import"
				enctype="multipart/form-data"
				class="flex flex-col gap-3"
				use:enhance={() => {
					importing = true;
					return async ({ update }) => {
						await update({ reset: false });
						importing = false;
					};
				}}
			>
				<div class="flex flex-wrap items-end gap-3">
					<Field label="Organization" for="orgId">
						<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
					</Field>
					<Field label="Key" for="key" hint="Optional; made from the API's title">
						<Input id="key" name="key" pattern="[a-z][a-z0-9-]*" maxlength={64} />
					</Field>
					<Field label="Spec file" for="file">
						<input
							id="file"
							name="file"
							type="file"
							accept=".json,.yaml,.yml,application/json,application/yaml"
							class="text-sm text-fg"
						/>
					</Field>
				</div>
				<p class="text-sm font-medium text-fg">Or paste the spec</p>
				<!-- JSON is YAML too, so one mode suits both. -->
				<CodeEditor id="spec" name="spec" label="Or paste the spec" language="yaml" lines={10} />
				<div>
					<Button type="submit" disabled={importing}>
						{importing ? 'Importing…' : 'Import'}
					</Button>
				</div>
			</form>
			{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		</Card>
	{/if}
</div>
