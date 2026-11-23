<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('team:manage'));
	let canAssign = $derived(data.user.grantedPermissions.includes('role_assignment:manage'));
	let nonMembers = $derived(
		data.users.filter((u) => !data.team.members.some((m) => m.userId === u.id))
	);
	let userId = $state('');
	let roleId = $state('');
	let orgId = $state('');

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update();
			if (result.type === 'success' || result.type === 'failure') {
				toast.push({
					title: String(result.data?.message ?? 'Done'),
					tone: result.type === 'success' ? 'success' : 'danger'
				});
			}
			userId = '';
			roleId = '';
			orgId = '';
		};
	};
</script>

<svelte:head>
	<title>{data.team.name} · Teams · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/teams')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All teams
	</a>

	<Card title={data.team.name} description={data.team.description || undefined}>
		{#if canManage}
			<form
				method="POST"
				action="?/update"
				class="flex flex-wrap items-end gap-2"
				use:enhance={notify}
			>
				<Field label="Team name" for="name">
					<Input id="name" name="name" value={data.team.name} required maxlength={100} />
				</Field>
				<Field label="Description" for="description">
					<Input
						id="description"
						name="description"
						value={data.team.description}
						maxlength={500}
					/>
				</Field>
				<Button type="submit">Save</Button>
			</form>
		{/if}
	</Card>

	<Card
		title="Members"
		description="Members get the team's roles immediately, and lose them when removed."
	>
		<div class="flex flex-col gap-4">
			{#if data.team.members.length === 0}
				<p class="text-sm text-fg-muted">No members yet.</p>
			{:else}
				<ul class="flex flex-col divide-y divide-border">
					{#each data.team.members as member (member.userId)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<p class="flex items-center gap-2 text-sm text-fg">
								{member.displayName}
								<span class="text-fg-muted">
									{member.kind === 'service' ? 'Service account' : member.email}
								</span>
							</p>
							{#if canManage}
								<form method="POST" action="?/removeMember" use:enhance={notify}>
									<input type="hidden" name="userId" value={member.userId} />
									<Button type="submit" size="sm" aria-label="Remove {member.displayName}">
										Remove
									</Button>
								</form>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if canManage && nonMembers.length > 0}
				<form method="POST" action="?/addMember" class="flex items-end gap-2" use:enhance={notify}>
					<Field label="Add member" for="userId">
						<Select
							id="userId"
							name="userId"
							bind:value={userId}
							required
							placeholder="Choose…"
							options={nonMembers.map((u) => ({
								value: u.id,
								label: `${u.displayName} (${u.kind === 'service' ? 'service account' : u.email})`
							}))}
						/>
					</Field>
					<Button type="submit" variant="primary" disabled={!userId}>Add</Button>
				</form>
			{/if}
		</div>
	</Card>

	<Card title="Roles" description="What members can do through this team, and where.">
		<div class="flex flex-col gap-4">
			{#if data.team.assignments.length === 0}
				<p class="text-sm text-fg-muted">No roles yet.</p>
			{:else}
				<ul class="flex flex-col divide-y divide-border">
					{#each data.team.assignments as a (a.id)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<p class="flex items-center gap-2 text-sm text-fg">
								<Badge tone="accent">{a.roleName}</Badge> at {a.orgName}{a.inherit
									? ' and below'
									: ''}
							</p>
							{#if canAssign}
								<form method="POST" action="?/unassign" use:enhance={notify}>
									<input type="hidden" name="id" value={a.id} />
									<Button type="submit" size="sm">Remove</Button>
								</form>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if canAssign}
				<form
					method="POST"
					action="?/assign"
					class="flex flex-wrap items-end gap-2"
					use:enhance={notify}
				>
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
					<Button type="submit" variant="primary" disabled={!roleId || !orgId}>Assign role</Button>
				</form>
			{/if}
		</div>
	</Card>

	{#if canManage}
		<Card title="Delete team" description="Members lose the team's roles; their own roles stay.">
			<form
				method="POST"
				action="?/delete"
				use:enhance={({ cancel }) => {
					if (!confirm(`Delete ${data.team.name}?`)) cancel();
				}}
			>
				<Button type="submit" variant="danger">Delete team</Button>
			</form>
		</Card>
	{/if}
</div>
