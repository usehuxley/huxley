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
