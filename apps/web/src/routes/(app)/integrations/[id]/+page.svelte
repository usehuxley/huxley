<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import { fieldsFor } from '#lib/integrations/connections.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let integration = $derived(data.integration);
	let definition = $derived(integration.definition);
	let canManage = $derived(data.user.grantedPermissions.includes('integration:manage'));
	let updating = $state(false);
	let canConnect = $derived(data.user.grantedPermissions.includes('connection:manage'));
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	let connectOrg = $state('');
	$effect(() => {
		if (!connectOrg) connectOrg = integration.orgId;
	});
	let schemeKey = $state('');
	$effect(() => {
		if (!schemeKey) schemeKey = definition.authSchemes[0]?.key ?? '';
	});
	let schemeOptions = $derived([
		...definition.authSchemes.map((s) => ({
			value: s.key,
			label: `${describeAuth(s)} (${s.key})`
		})),
		{ value: '', label: 'No authentication' }
	]);
	let schemeFields = $derived(
		fieldsFor(definition.authSchemes.find((s) => s.key === schemeKey)?.type)
	);
	let serverVariables = $derived(definition.servers[0]?.variables ?? []);
	const statusTone = (status: string) =>
		status === 'ready' ? 'success' : status === 'needs_auth' ? 'warning' : 'danger';
	const statusLabel = (status: string) =>
		status === 'ready' ? 'Ready' : status === 'needs_auth' ? 'Needs sign-in' : 'Error';

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

	<Card
		title="Connections"
		description="Credentials for calling this API. Organizations below the owner can use shared ones; secrets are never shown again."
	>
		{#if data.signedIn}
			<p class="mb-3 text-sm text-fg" role="status">Signed in {data.signedIn}.</p>
		{/if}
		{#if data.connections.length === 0}
			<p class="text-sm text-fg-muted">No connections yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Connections">
				{#each data.connections as connection (connection.id)}
					<li class="flex items-center justify-between gap-4 py-2">
						<div class="min-w-0 text-sm">
							<p class="text-fg">
								{connection.name}
								{#if connection.shared}<Badge tone="neutral">Shared</Badge>{/if}
							</p>
							<p class="font-mono text-xs text-fg-muted">
								{connection.key} · {orgName.get(connection.orgId) ?? 'Unknown organization'}
								{#if connection.statusMessage}· {connection.statusMessage}{/if}
								· {connection.authScheme ?? 'no auth'}
								{#if connection.secretFields.length}· {connection.secretFields.join(', ')} set{/if}
							</p>
						</div>
						<div class="flex items-center gap-2">
							<Badge tone={statusTone(connection.status)}>{statusLabel(connection.status)}</Badge>
							{#if canConnect && connection.status === 'needs_auth'}
								<form method="POST" action="?/signIn">
									<input type="hidden" name="connectionId" value={connection.id} />
									<Button type="submit" size="sm">Sign in</Button>
								</form>
							{/if}
							{#if canConnect}
								<form method="POST" action="?/disconnect" use:enhance>
									<input type="hidden" name="connectionId" value={connection.id} />
									<Button type="submit" variant="ghost" size="sm">Delete</Button>
								</form>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
		{#if canConnect}
			<form
				method="POST"
				action="?/connect"
				class="mt-4 flex flex-col gap-3"
				use:enhance={() =>
					async ({ update }) => {
						await update({ reset: true });
					}}
				aria-label="New connection"
			>
				<div class="flex flex-wrap items-end gap-3">
					<Field label="Connection name" for="connName">
						<Input id="connName" name="name" required maxlength={200} />
					</Field>
					<Field label="Organization" for="connOrg">
						<Select id="connOrg" name="orgId" options={orgOptions} bind:value={connectOrg} />
					</Field>
					<Field label="Authentication" for="connScheme">
						<Select
							id="connScheme"
							name="authScheme"
							options={schemeOptions}
							bind:value={schemeKey}
						/>
					</Field>
				</div>
				{#if schemeFields.length}
					<div class="flex flex-wrap items-end gap-3">
						{#each schemeFields as field (field.name)}
							<Field label={field.label} for="conn-{field.name}" hint={field.hint}>
								<Input
									id="conn-{field.name}"
									name="field.{field.name}"
									type={field.secret ? 'password' : 'text'}
									autocomplete="off"
								/>
							</Field>
						{/each}
					</div>
				{/if}
				<div class="flex flex-wrap items-end gap-3">
					{#each serverVariables as variable (variable.name)}
						<Field
							label={variable.name}
							for="var-{variable.name}"
							hint={variable.description ?? undefined}
						>
							<Input
								id="var-{variable.name}"
								name="var.{variable.name}"
								placeholder={variable.default}
							/>
						</Field>
					{/each}
					<Field
						label="Base URL"
						for="connBase"
						hint={definition.servers.length ? 'Optional; overrides the spec' : 'Required'}
					>
						<Input
							id="connBase"
							name="baseUrl"
							type="url"
							placeholder={definition.servers[0]?.url}
						/>
					</Field>
				</div>
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="shared" /> Share with the organizations below
				</label>
				<div><Button type="submit">Create connection</Button></div>
			</form>
			{#if form && 'connectMessage' in form && form.connectMessage}
				<p class="mt-2 text-sm text-danger" role="alert">{form.connectMessage}</p>
			{/if}
		{/if}
	</Card>

	{#if form && 'preview' in form && form.preview}
		{@const preview = form.preview}
		{@const diff = preview.diff}
		<Card
			title="Review the update"
			description="Compared with version {preview.baseVersion}. Nothing changes until you apply it."
		>
			<div class="flex flex-col gap-3 text-sm" aria-label="Update review" role="region">
				{#if diff.breaking}
					<p class="rounded-control bg-danger/10 p-2 text-danger" role="alert">
						Some changes can break workflows or connections that use this integration.
					</p>
				{:else if diff.added.length + diff.changed.length + diff.connection.length === 0}
					<p class="text-fg-muted">No changes to actions, authentication or servers.</p>
				{:else}
					<p class="text-fg">Nothing here should break what already uses it.</p>
				{/if}
				{#if diff.removed.length}
					<div>
						<p class="font-medium text-danger">Removed actions</p>
						<p class="font-mono text-xs">{diff.removed.join(', ')}</p>
					</div>
				{/if}
				{#if diff.renamed.length}
					<div>
						<p class="font-medium text-danger">Renamed actions (same method and path)</p>
						<ul class="font-mono text-xs">
							{#each diff.renamed as r (r.from)}<li>{r.from} → {r.to}</li>{/each}
						</ul>
					</div>
				{/if}
				{#if diff.changed.length}
					<div>
						<p class="font-medium text-fg">Changed actions</p>
						<ul class="flex flex-col gap-1">
							{#each diff.changed as action (action.key)}
								<li>
									<span class="font-mono text-xs">{action.key}</span>
									<ul class="ml-4 list-disc">
										{#each action.changes as change, i (i)}
											<li class={change.breaking ? 'text-danger' : 'text-fg-muted'}>
												{change.message}
											</li>
										{/each}
									</ul>
								</li>
							{/each}
						</ul>
					</div>
				{/if}
				{#if diff.added.length}
					<div>
						<p class="font-medium text-fg">New actions</p>
						<p class="font-mono text-xs">{diff.added.join(', ')}</p>
					</div>
				{/if}
				{#if diff.connection.length}
					<div>
						<p class="font-medium text-fg">Authentication and servers</p>
						<ul class="ml-4 list-disc">
							{#each diff.connection as change, i (i)}
								<li class={change.breaking ? 'text-danger' : 'text-fg-muted'}>
									{change.message}
								</li>
							{/each}
						</ul>
					</div>
				{/if}
				{#if preview.warnings.length}
					<p class="text-fg-muted">
						{preview.warnings.length} import notes, shown once it's applied.
					</p>
				{/if}
				<form method="POST" action="?/update" use:enhance class="flex gap-2">
					<input type="hidden" name="baseVersion" value={preview.baseVersion} />
					<textarea name="spec" hidden>{form.spec}</textarea>
					<Button type="submit" variant={diff.breaking ? 'danger' : 'primary'}>
						Apply as a new version
					</Button>
					<a
						href={resolve('/(app)/integrations/[id]', { id: integration.id })}
						class="self-center text-sm text-fg-muted hover:text-fg"
					>
						Cancel
					</a>
				</form>
			</div>
		</Card>
	{/if}

	<Card title="Versions">
		<ul class="flex flex-col gap-2 text-sm" aria-label="Versions">
			{#each data.versions as version (version.version)}
				<li class="flex items-center justify-between gap-2">
					<span>
						<span class="font-medium text-fg">Version {version.version}</span>
						<span class="text-fg-muted">
							· {version.actions} actions
							{#if version.warnings}· {version.warnings} notes{/if}
							· {when(version.importedAt)}
						</span>
					</span>
					{#if version.version === integration.currentVersion}
						<Badge tone="success">Current</Badge>
					{:else if canManage}
						<form method="POST" action="?/setCurrent" use:enhance>
							<input type="hidden" name="version" value={version.version} />
							<Button type="submit" variant="ghost" size="sm">Make current</Button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
		{#if canManage}
			<form
				method="POST"
				action="?/preview"
				enctype="multipart/form-data"
				class="mt-4 flex flex-col gap-2"
				use:enhance={() => {
					updating = true;
					return async ({ update }) => {
						await update({ reset: false });
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
						{updating ? 'Reading…' : 'Review changes'}
					</Button>
				</div>
			</form>
		{/if}
		{#if form && 'message' in form && form.message}
			<p class="mt-2 text-sm text-fg" role="status">{form.message}</p>
		{/if}
	</Card>
</div>
