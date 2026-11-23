<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input } from '@usehuxley/ui';
	import PermissionPicker from '#lib/roles/PermissionPicker.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('role:manage'));
	let creating = $state(false);
	const summary = (perms: string[]) =>
		perms.includes('*')
			? 'Every permission'
			: `${perms.length} permission${perms.length === 1 ? '' : 's'}`;
</script>

<svelte:head>
	<title>Roles · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Roles"
		description="What people can do. Assign roles to people or teams at an organization."
	>
		{#snippet actions()}
			{#if canManage && !creating}
				<Button size="sm" variant="primary" onclick={() => (creating = true)}>New role</Button>
			{/if}
		{/snippet}
		<ul class="flex flex-col divide-y divide-border">
			{#each data.roles as role (role.id)}
				<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-medium text-fg">
							{#if role.isSystem}
								{role.name}
								<Badge>Built in</Badge>
							{:else}
								<a
									href={resolve('/(app)/settings/roles/[id]', { id: role.id })}
									class="hover:underline"
								>
									{role.name}
								</a>
								<Badge tone="accent">Custom</Badge>
							{/if}
						</p>
						<p class="mt-0.5 text-sm text-fg-muted">
							{role.description || summary(role.permissions)}
						</p>
					</div>
					<span class="text-sm text-fg-muted">{summary(role.permissions)}</span>
				</li>
			{/each}
		</ul>
	</Card>

	{#if canManage && (creating || form?.message)}
		<Card title="New role">
			<form method="POST" action="?/create" class="flex flex-col gap-4" use:enhance>
				{#if form?.message}<p role="alert" class="text-sm text-danger">{form.message}</p>{/if}
				<div class="grid gap-4 sm:grid-cols-2">
					<Field label="Role name" for="roleName">
						<Input
							id="roleName"
							name="name"
							required
							maxlength={100}
							placeholder="e.g. Dispatcher"
						/>
					</Field>
					<Field label="Description" for="roleDescription">
						<Input id="roleDescription" name="description" maxlength={500} />
					</Field>
				</div>
				<PermissionPicker permissions={data.permissions} held={data.user.grantedPermissions} />
				<div class="flex gap-2">
					<Button type="submit" variant="primary">Create role</Button>
					<Button type="button" onclick={() => (creating = false)}>Cancel</Button>
				</div>
			</form>
		</Card>
	{/if}
</div>
