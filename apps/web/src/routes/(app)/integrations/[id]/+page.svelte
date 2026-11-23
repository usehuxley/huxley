<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let integration = $derived(data.integration);
	let definition = $derived(integration.definition);
	let canManage = $derived(data.user.grantedPermissions.includes('integration:manage'));
	let updating = $state(false);

	type Action = (typeof definition.actions)[number];
	// Actions grouped by their first tag, groups in spec order.
	let groups = $derived.by(() => {
		const byGroup: [string, Action[]][] = [];
		for (const action of definition.actions) {
			const name = action.group ?? 'Other';
			const found = byGroup.find(([g]) => g === name);
			if (found) found[1].push(action);
			else byGroup.push([name, [action]]);
		}
		return byGroup;
	});

	const methodTone = (method: string) =>
		method === 'GET'
			? 'info'
			: method === 'DELETE'
				? 'danger'
				: method === 'POST'
					? 'success'
					: 'neutral';

	function describeAuth(scheme: (typeof definition.authSchemes)[number]): string {
		switch (scheme.type) {
			case 'apiKey':
				return `API key in ${scheme.location} ${scheme.name}`;
			case 'basic':
				return 'Username and password (HTTP basic)';
			case 'bearer':
				return `Bearer token${scheme.format ? ` (${scheme.format})` : ''}`;
			case 'oAuth2ClientCredentials':
				return 'OAuth 2.0 client credentials';
			case 'oAuth2AuthorizationCode':
				return 'OAuth 2.0 sign-in (authorization code)';
			case 'openIdConnect':
				return 'OpenID Connect';
		}
	}
	function when(value: string): string {
		return new Date(value).toLocaleString();
	}
</script>

<svelte:head>
	<title>{integration.name} · Integrations · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/integrations')}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} /> Integrations
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{integration.name}</h1>
		<p class="mt-1 text-sm text-fg-muted">
			<span class="font-mono">{integration.key}</span>
			· version {integration.currentVersion}
			{#if definition.version}· API {definition.version}{/if}
			· from {definition.sourceFormat === '2.0'
				? 'Swagger 2.0'
				: `OpenAPI ${definition.sourceFormat}`}
		</p>
		{#if definition.description}
			<p class="mt-2 text-sm text-fg">{definition.description}</p>
		{/if}
	</header>

	{#if integration.warnings.length > 0}
		<Card
			title="Import notes"
			description="What the importer worked around in this spec. The actions still work."
		>
			<ul class="flex flex-col gap-1 text-sm" aria-label="Import warnings">
				{#each integration.warnings as warning, i (i)}
					<li>
						<span class="text-fg">{warning.message}</span>
						<span class="font-mono text-xs text-fg-muted">{warning.pointer}</span>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	<Card title="Actions" description="{definition.actions.length} operations workflows can call.">
		{#each groups as [group, actions] (group)}
			<section class="mb-4 last:mb-0">
				<h2 class="mb-2 text-sm font-semibold text-fg">{group}</h2>
				<ul class="flex flex-col divide-y divide-border" aria-label="{group} actions">
					{#each actions as action (action.key)}
						<li class="flex items-start justify-between gap-4 py-2">
							<div class="min-w-0">
								<p class="text-sm text-fg">
									{action.name}
									{#if action.deprecated}<Badge tone="warning">Deprecated</Badge>{/if}
								</p>
								<p class="mt-0.5 font-mono text-xs break-all text-fg-muted">
									{action.key} · {action.path}
								</p>
							</div>
							<Badge tone={methodTone(action.method)}>{action.method}</Badge>
						</li>
					{/each}
				</ul>
			</section>
		{/each}
	</Card>

	<div class="grid gap-6 md:grid-cols-2">
		<Card title="Authentication">
			{#if definition.authSchemes.length === 0}
				<p class="text-sm text-fg-muted">The spec doesn't describe any.</p>
			{:else}
				<ul class="flex flex-col gap-2 text-sm">
					{#each definition.authSchemes as scheme (scheme.key)}
						<li>
							<span class="text-fg">{describeAuth(scheme)}</span>
							<span class="font-mono text-xs text-fg-muted">{scheme.key}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
		<Card title="Servers">
			{#if definition.servers.length === 0}
				<p class="text-sm text-fg-muted">None in the spec: connections give the base URL.</p>
			{:else}
				<ul class="flex flex-col gap-2 text-sm">
					{#each definition.servers as server (server.url)}
						<li>
							<span class="font-mono text-xs break-all text-fg">{server.url}</span>
							{#if server.description}<p class="text-fg-muted">{server.description}</p>{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</Card>
	</div>

	<Card title="Versions">
		<ul class="flex flex-col gap-2 text-sm" aria-label="Versions">
			{#each data.versions as version (version.version)}
				<li>
					<span class="font-medium text-fg">Version {version.version}</span>
					<span class="text-fg-muted">
						· {version.actions} actions
						{#if version.warnings}· {version.warnings} notes{/if}
						· {when(version.importedAt)}
					</span>
				</li>
			{/each}
		</ul>
		{#if canManage}
			<form
				method="POST"
				action="?/update"
				enctype="multipart/form-data"
				class="mt-4 flex flex-col gap-2"
				use:enhance={() => {
					updating = true;
					return async ({ update }) => {
						await update({ reset: true });
						updating = false;
					};
				}}
			>
				<label for="file" class="text-sm font-medium text-fg">Import a newer spec</label>
				<input id="file" name="file" type="file" accept=".json,.yaml,.yml" class="text-sm" />
				<textarea
					name="spec"
					rows="4"
					aria-label="Or paste the newer spec"
					spellcheck="false"
					class="rounded-control border border-border bg-surface p-2 font-mono text-xs text-fg"
				></textarea>
				<div>
					<Button type="submit" variant="secondary" disabled={updating}>
						{updating ? 'Importing…' : 'Import as a new version'}
					</Button>
				</div>
			</form>
			{#if form?.message}<p class="mt-2 text-sm text-fg" role="status">{form.message}</p>{/if}
		{/if}
	</Card>
</div>
