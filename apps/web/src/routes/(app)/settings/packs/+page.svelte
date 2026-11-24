<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input } from '@usehuxley/ui';
	import { Package } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let review = $derived(form && 'review' in form ? form.review : undefined);
	let conflicts = $derived(review?.items.filter((i) => i.action === 'conflict') ?? []);
	const kinds: Record<string, string> = {
		integrations: 'Integration',
		workflows: 'Workflow (as a draft)',
		forms: 'Form',
		scripts: 'Script',
		templates: 'Template'
	};
	const actions: Record<string, string> = {
		add: 'Adds',
		update: 'Updates',
		conflict: 'Conflict'
	};
	const trustLabels: Record<string, string> = {
		built_in: 'Built in',
		trusted: 'Signed, trusted',
		unknown_signer: 'Signed by an unknown key',
		unsigned: 'Unsigned'
	};
	const trustLabel = (status: string) => trustLabels[status] ?? status;
	const trustTone = (status: string) =>
		status === 'trusted' || status === 'built_in' ? 'success' : 'warning';
	let installedKeys = $derived(new Set(data.installed.map((p) => p.key)));
	const exportKinds = [
		['integrations', 'Integrations'],
		['workflows', 'Workflows (their drafts)'],
		['forms', 'Forms'],
		['scripts', 'Scripts']
	] as const;
	let hasExportable = $derived(exportKinds.some(([k]) => data.exportable[k].length > 0));
</script>

<svelte:head>
	<title>Packs · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h2 class="text-xl font-semibold text-fg">Packs</h2>
			<p class="mt-1 text-sm text-fg-muted">
				Packs bundle integrations, workflows, forms, scripts and templates to share between
				instances.
			</p>
		</div>
		{#if data.orgs.length > 1}
			<form method="GET" class="flex items-end gap-2">
				<Field label="Organization" for="pack-org">
					<select
						id="pack-org"
						name="org"
						class="rounded-control border border-border bg-surface px-2 py-1.5 text-sm"
						onchange={(e) => e.currentTarget.form?.requestSubmit()}
					>
						{#each data.orgs as org (org.id)}
							<option value={org.id} selected={org.id === data.orgId}>{org.name}</option>
						{/each}
					</select>
				</Field>
			</form>
		{/if}
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg" role="status">{form.message}</p>
	{/if}

	{#if review}
		<Card title="Review {review.name} {review.version}" description={review.description}>
			<div class="flex flex-col gap-4 text-sm" data-testid="pack-review">
				<p class="flex flex-wrap items-center gap-2">
					<span class="font-mono text-xs text-fg-muted">{review.key}</span>
					{#if review.publisher}<span>by {review.publisher}</span>{/if}
					<Badge tone={trustTone(review.trust.status)}>{trustLabel(review.trust.status)}</Badge>
					{#if review.installedVersion}
						<Badge tone="info">Upgrades {review.installedVersion}</Badge>
					{/if}
				</p>
				{#if review.trust.status !== 'trusted' && review.trust.status !== 'built_in'}
					<p class="rounded-control bg-warning-soft p-3 text-warning" role="alert">
						{#if review.trust.status === 'unsigned'}
							Nobody vouches for this pack. Install it only if you trust where it came from.
						{:else}
							It's signed, but with a key this instance doesn't trust:
							<code class="font-mono text-xs break-all">{review.trust.key}</code>. Check it with the
							publisher before installing.
						{/if}
					</p>
				{/if}

				<div>
					<h3 class="font-medium text-fg">What it adds</h3>
					<ul class="mt-1 flex flex-col divide-y divide-border" aria-label="Pack contents">
						{#each review.items as item, i (i)}
							<li class="flex items-center justify-between gap-4 py-2">
								<span>
									{kinds[item.kind] ?? item.kind} <code class="font-mono text-xs">{item.key}</code>
								</span>
								<Badge
									tone={item.action === 'conflict'
										? 'danger'
										: item.action === 'update'
											? 'info'
											: 'neutral'}
								>
									{actions[item.action] ?? item.action}
								</Badge>
							</li>
						{/each}
					</ul>
				</div>

				<div>
					<h3 class="font-medium text-fg">Where it connects</h3>
					{#if review.egress.length}
						<ul class="mt-1 flex flex-wrap gap-1" aria-label="Egress domains">
							{#each review.egress as host (host)}
								<li>
									<code class="rounded bg-surface-sunken px-1.5 py-0.5 font-mono text-xs">
										{host}
									</code>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="mt-1 text-fg-muted">No fixed hosts.</p>
					{/if}
				</div>

				{#if review.scripts.length}
					<div>
						<h3 class="font-medium text-fg">Scripts it adds</h3>
						<p class="mt-1 text-fg-muted">
							{review.scripts.map((s) => `${s.key} (${s.language})`).join(', ')}. They run in
							sandboxes on your runners.
						</p>
					</div>
				{/if}

				{#if conflicts.length}
					<p class="text-danger" role="alert">
						Something else here already uses
						{conflicts.map((c) => c.key).join(', ')}. Rename or remove it to install this pack:
						packs never overwrite what they didn't install.
					</p>
				{/if}

				<form method="POST" action="?/install" use:enhance class="flex gap-2">
					<input type="hidden" name="uploadId" value={review.uploadId} />
					<input type="hidden" name="orgId" value={data.orgId} />
					<input type="hidden" name="version" value={review.version} />
					<Button type="submit" disabled={conflicts.length > 0}>
						{review.installedVersion ? 'Upgrade' : 'Install'}
					</Button>
				</form>
			</div>
		</Card>
	{/if}

	<Card title="Installed here">
		{#if data.installed.length === 0}
			<EmptyState title="No packs installed" description="Upload one below.">
				{#snippet icon()}<Package size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Installed packs">
				{#each data.installed as pack (pack.key)}
					<li class="flex items-center justify-between gap-4 py-3 text-sm first:pt-0 last:pb-0">
						<div>
							<p class="font-medium text-fg">{pack.name} {pack.version}</p>
							<p class="text-xs text-fg-muted">
								<span class="font-mono">{pack.key}</span>
								{#if pack.publisher}· {pack.publisher}{/if}
								· installed {new Date(pack.installedAt).toLocaleString()}
							</p>
						</div>
						<Badge tone={trustTone(pack.trust)}>{trustLabel(pack.trust)}</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if data.builtin.length}
		<Card title="Built-in packs" description="Shipped with Huxley.">
			<ul class="flex flex-col divide-y divide-border" aria-label="Built-in packs">
				{#each data.builtin as pack (pack.key)}
					<li class="flex items-center justify-between gap-4 py-3 text-sm first:pt-0 last:pb-0">
						<div>
							<p class="font-medium text-fg">{pack.name} {pack.version}</p>
							<p class="text-xs text-fg-muted">{pack.description}</p>
						</div>
						<form method="POST" action="?/reviewBuiltin" use:enhance>
							<input type="hidden" name="orgId" value={data.orgId} />
							<input type="hidden" name="key" value={pack.key} />
							<Button type="submit" variant="secondary" size="sm" aria-label="Review {pack.name}">
								{installedKeys.has(pack.key) ? 'Review again' : 'Review'}
							</Button>
						</form>
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	<Card
		title="Install a pack"
		description="Upload a .hxpack file. You'll see what it adds before anything changes."
	>
		<form
			method="POST"
			action="?/upload"
			enctype="multipart/form-data"
			use:enhance
			class="flex flex-wrap items-end gap-3"
		>
			<input type="hidden" name="orgId" value={data.orgId} />
			<Field label="Pack file" for="pack-file">
				<input id="pack-file" name="pack" type="file" accept=".hxpack" required class="text-sm" />
			</Field>
			<Button type="submit" variant="secondary">Review</Button>
		</form>
	</Card>

	<Card
		title="Export a pack"
		description="Bundle this organization's items to install elsewhere. Connections and secrets are never included. Sign it with huxley pack sign."
	>
		{#if !hasExportable}
			<p class="text-sm text-fg-muted">Nothing here to export yet.</p>
		{:else}
			<form
				method="POST"
				action={resolve('/(app)/settings/packs/export')}
				class="flex flex-col gap-4"
			>
				<input type="hidden" name="orgId" value={data.orgId} />
				<div class="grid gap-3 sm:grid-cols-2">
					<Field label="Pack key" for="export-key" hint="owner/name, lowercase">
						<Input id="export-key" name="key" required pattern="[a-z][a-z0-9-]*/[a-z][a-z0-9-]*" />
					</Field>
					<Field label="Pack name" for="export-name">
						<Input id="export-name" name="name" required maxlength={200} />
					</Field>
					<Field label="Version" for="export-version" hint="e.g. 1.0.0">
						<Input id="export-version" name="version" required value="1.0.0" />
					</Field>
					<Field label="Publisher" for="export-publisher">
						<Input id="export-publisher" name="publisher" maxlength={200} />
					</Field>
				</div>
				<Field label="Description" for="export-description">
					<Input id="export-description" name="description" maxlength={2000} />
				</Field>
				{#each exportKinds as [kind, label] (kind)}
					{#if data.exportable[kind].length}
						<fieldset class="text-sm">
							<legend class="font-medium text-fg">{label}</legend>
							<div class="mt-1 flex flex-wrap gap-x-4 gap-y-1">
								{#each data.exportable[kind] as item (item.id)}
									<label class="flex items-center gap-1.5">
										<input type="checkbox" name={kind} value={item.id} />
										{item.name} <span class="font-mono text-xs text-fg-muted">{item.key}</span>
									</label>
								{/each}
							</div>
						</fieldset>
					{/if}
				{/each}
				<div><Button type="submit" variant="secondary">Download pack</Button></div>
			</form>
		{/if}
	</Card>
</div>
