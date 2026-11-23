<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import PermissionPicker from '#lib/roles/PermissionPicker.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('role:manage'));
</script>

<svelte:head>
	<title>{data.role.name} · Roles · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/roles')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All roles
	</a>

	<Card
		title={data.role.name}
		description="Changes apply to everyone holding this role at their next request."
	>
		<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance>
			{#if form && 'message' in form && form.message}<p role="alert" class="text-sm text-danger">
					{form.message}
				</p>{/if}
			{#if form && 'saved' in form}<p role="status" class="text-sm text-success">Saved.</p>{/if}
			<div class="grid gap-4 sm:grid-cols-2">
				<Field label="Role name" for="roleName">
					<Input
						id="roleName"
						name="name"
						value={data.role.name}
						required
						maxlength={100}
						disabled={!canManage}
					/>
				</Field>
				<Field label="Description" for="roleDescription">
					<Input
						id="roleDescription"
						name="description"
						value={data.role.description}
						maxlength={500}
						disabled={!canManage}
					/>
				</Field>
			</div>
			<PermissionPicker
				permissions={data.permissions}
				selected={data.role.permissions}
				held={canManage ? data.user.grantedPermissions : []}
			/>
			{#if canManage}<div><Button type="submit" variant="primary">Save role</Button></div>{/if}
		</form>
	</Card>

	{#if canManage}
		<Card title="Delete role" description="Only roles nobody holds can be deleted.">
			<form
				method="POST"
				action="?/delete"
				use:enhance={({ cancel }) => {
					if (!confirm(`Delete ${data.role.name}?`)) cancel();
				}}
			>
				<Button type="submit" variant="danger">Delete role</Button>
			</form>
		</Card>
	{/if}
</div>
