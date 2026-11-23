<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, Field, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let root = $derived(data.orgs.find((o) => o.parentId === null));
	let requiredRoles = $derived(new Set(data.policy.roleIds));
	let requiredOrgs = $derived(data.policy.orgs.filter((o) => !o.isRoot));
	let otherOrgs = $derived(
		data.orgs.filter((o) => o.parentId !== null && !requiredOrgs.some((r) => r.id === o.id))
	);
	let orgToAdd = $state('');

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update();
			orgToAdd = '';
			const message = String(
				(result.type === 'success' || result.type === 'failure') && result.data?.message
					? result.data.message
					: 'Saved'
			);
			toast.push({ title: message, tone: result.type === 'success' ? 'success' : 'danger' });
		};
	};
</script>

<svelte:head>
	<title>Security · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<p class="text-sm text-fg-muted">
		People covered by a requirement who have no passkey or authenticator app can only set one up
		until they do. That includes you, if you turn on a requirement that covers you.
	</p>

	<Card
		title="Require two-factor authentication for everyone"
		description="Every user must have a passkey or an authenticator app."
	>
		{#snippet actions()}
			<Badge tone={data.policy.everyone ? 'success' : 'neutral'}>
				{data.policy.everyone ? 'Required' : 'Not required'}
			</Badge>
		{/snippet}
		{#if root}
			<form method="POST" action="?/setOrg" use:enhance={notify}>
				<input type="hidden" name="id" value={root.id} />
				<input type="hidden" name="required" value={String(!data.policy.everyone)} />
				<Button type="submit" variant={data.policy.everyone ? 'secondary' : 'primary'}>
					{data.policy.everyone ? 'Stop requiring for everyone' : 'Require for everyone'}
				</Button>
			</form>
		{/if}
	</Card>

	<Card title="By role" description="Everyone holding the role, at any organization.">
		<ul class="flex flex-col divide-y divide-border">
			{#each data.roles as role (role.id)}
				{@const required = requiredRoles.has(role.id)}
				<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-medium text-fg">
							{role.name}
							{#if required}<Badge tone="success">Required</Badge>{/if}
						</p>
						<p class="mt-0.5 text-sm text-fg-muted">{role.description}</p>
					</div>
					<form method="POST" action="?/setRole" use:enhance={notify}>
						<input type="hidden" name="id" value={role.id} />
						<input type="hidden" name="required" value={String(!required)} />
						<Button
							type="submit"
							size="sm"
							aria-label="{required ? 'Stop requiring' : 'Require'} for {role.name}"
						>
							{required ? 'Stop requiring' : 'Require'}
						</Button>
					</form>
				</li>
			{/each}
		</ul>
	</Card>

	<Card
		title="By organization"
		description="Everyone who can reach the organization: assigned at it or below it, or above it with access to sub-organizations."
	>
		<div class="flex flex-col gap-4">
			{#if requiredOrgs.length > 0}
				<ul class="flex flex-col divide-y divide-border">
					{#each requiredOrgs as org (org.id)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<p class="text-sm font-medium text-fg">{org.name}</p>
							<form method="POST" action="?/setOrg" use:enhance={notify}>
								<input type="hidden" name="id" value={org.id} />
								<input type="hidden" name="required" value="false" />
								<Button type="submit" size="sm" aria-label="Stop requiring for {org.name}">
									Stop requiring
								</Button>
							</form>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="text-sm text-fg-muted">No organization-specific requirements.</p>
			{/if}
			{#if otherOrgs.length > 0}
				<form method="POST" action="?/setOrg" class="flex items-end gap-2" use:enhance={notify}>
					<input type="hidden" name="required" value="true" />
					<Field label="Organization" for="orgToAdd">
						<Select
							id="orgToAdd"
							name="id"
							bind:value={orgToAdd}
							required
							placeholder="Choose…"
							options={otherOrgs.map((o) => ({ value: o.id, label: o.name }))}
						/>
					</Field>
					<Button type="submit" variant="primary" disabled={!orgToAdd}>Require</Button>
				</form>
			{/if}
		</div>
	</Card>
</div>
