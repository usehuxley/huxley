<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import ProviderFields from '../ProviderFields.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let errors = $derived((form && 'errors' in form ? form.errors : {}) as Record<string, string>);
	let roleName = (id: string) => data.roles.find((r) => r.id === id)?.name ?? 'Unknown role';
	let orgName = (id: string) => data.orgs.find((o) => o.id === id)?.name ?? 'Unknown organization';
	let roleId = $state('');
	let orgId = $state('');

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update({ reset: result.type === 'success' });
			if (result.type === 'success' && result.data?.message) {
				toast.push({ title: String(result.data.message), tone: 'success' });
			}
		};
	};
</script>

<svelte:head>
	<title>{data.provider.name} · Single sign-on · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/sso')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All identity providers
	</a>

	<Card
		title={data.provider.name}
		description={data.provider.redirectUri
			? `Redirect URI to register with the provider: ${data.provider.redirectUri}`
			: 'Set auth.public_url on the server to enable single sign-on.'}
	>
		<form method="POST" action="?/update" use:enhance={notify}>
			{#if form && 'message' in form && form.message && Object.keys(errors).length > 0}
				<p role="alert" class="mb-4 text-sm text-danger">{form.message}</p>
			{/if}
			<ProviderFields provider={data.provider} {errors} />
			<div class="mt-6"><Button type="submit" variant="primary">Save</Button></div>
		</form>
	</Card>

	<Card
		title="Role mappings"
		description={data.provider.roleClaim
			? `Values of the “${data.provider.roleClaim}” claim grant roles. Synced at each sign-in; roles granted by hand are never removed.`
			: 'Set a role claim above to map claim values to roles.'}
	>
		<div class="flex flex-col gap-4">
			{#if form && 'mappingError' in form}
				<p role="alert" class="text-sm text-danger">{form.mappingError}</p>
			{/if}
			{#if data.provider.roleMappings.length > 0}
				<ul class="flex flex-col divide-y divide-border">
					{#each data.provider.roleMappings as mapping, index (index)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<p class="text-sm text-fg">
								<code class="font-mono">{mapping.claimValue}</code> → {roleName(mapping.roleId)} at
								{orgName(mapping.orgId)}{mapping.inherit ? ' and below' : ''}
							</p>
							<form method="POST" action="?/removeMapping" use:enhance={notify}>
								<input type="hidden" name="index" value={index} />
								<Button type="submit" size="sm">Remove</Button>
							</form>
						</li>
					{/each}
				</ul>
			{/if}
			<form
				method="POST"
				action="?/addMapping"
				class="flex flex-wrap items-end gap-2"
				use:enhance={notify}
			>
				<Field label="Claim value" for="claimValue">
					<Input id="claimValue" name="claimValue" required placeholder="Group id or app role" />
				</Field>
				<Field label="Role" for="roleId">
					<Select
						id="roleId"
						name="roleId"
						bind:value={roleId}
						required
						placeholder="Choose…"
						options={data.roles.map((r) => ({ value: r.id, label: r.name }))}
					/>
				</Field>
				<Field label="At" for="orgId">
					<Select
						id="orgId"
						name="orgId"
						bind:value={orgId}
						required
						placeholder="Choose…"
						options={data.orgs.map((o) => ({ value: o.id, label: o.name }))}
					/>
				</Field>
				<label class="flex h-9 items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="inherit" checked /> And below
				</label>
				<Button type="submit" variant="primary">Add mapping</Button>
			</form>
		</div>
	</Card>

	<Card
		title="Provisioning (SCIM)"
		description="Let {data.provider
			.name} create, update and deactivate people here, and manage the members of teams it syncs as groups. Deactivated or removed people are disabled and signed out; their history stays. Roles are still given to teams here."
	>
		{#if data.scim}
			{@const token = form && 'scimToken' in form ? String(form.scimToken) : null}
			{@const url = data.scim.url ?? (form && 'scimUrl' in form ? form.scimUrl : null)}
			<div class="flex flex-col gap-4 text-sm">
				<dl class="grid gap-x-6 gap-y-2 sm:grid-cols-[max-content_1fr]">
					<dt class="text-fg-muted">Tenant URL</dt>
					<dd class="font-mono break-all text-fg" data-testid="scim-url">
						{url ?? 'Set auth.public_url to show it'}
					</dd>
					<dt class="text-fg-muted">Status</dt>
					<dd class="text-fg" data-testid="scim-status">
						{#if data.scim.enabled}
							On · {data.scim.users}
							{data.scim.users === 1 ? 'person' : 'people'} and {data.scim.groups}
							{data.scim.groups === 1 ? 'team' : 'teams'} provisioned{data.scim.lastUsedAt
								? ` · last used ${new Date(data.scim.lastUsedAt).toLocaleString()}`
								: ' · not used yet'}
						{:else}
							Off
						{/if}
					</dd>
				</dl>
				{#if token}
					<div class="rounded-md border border-border bg-surface-sunken p-3" role="status">
						<p class="font-medium text-fg">Copy the secret token now. It isn’t shown again.</p>
						<p class="mt-2 font-mono break-all text-fg" data-testid="scim-token">{token}</p>
					</div>
				{/if}
				<div class="flex flex-wrap gap-2">
					<form
						method="POST"
						action="?/scimToken"
						use:enhance={({ cancel }) => {
							if (
								data.scim?.enabled &&
								!confirm('Make a new token? The current one stops working straight away.')
							)
								cancel();
							return async ({ update }) => update({ reset: false });
						}}
					>
						<Button type="submit" variant={data.scim.enabled ? 'secondary' : 'primary'}>
							{data.scim.enabled ? 'Make a new token' : 'Turn on provisioning'}
						</Button>
					</form>
					{#if data.scim.enabled}
						<form method="POST" action="?/scimDisable" use:enhance={notify}>
							<Button type="submit">Turn off</Button>
						</form>
					{/if}
				</div>
			</div>
		{/if}
	</Card>

	<Card
		title="Delete provider"
		description="Removes the provider, the identities linked to it, and the roles its mappings granted. Users are kept."
	>
		<form
			method="POST"
			action="?/delete"
			use:enhance={({ cancel }) => {
				if (!confirm(`Delete ${data.provider.name}? People can no longer sign in with it.`))
					cancel();
			}}
		>
			<Button type="submit" variant="danger">Delete provider</Button>
		</form>
	</Card>
</div>
