<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let roleId = $state('');
	let orgId = $state('');

	let isSelf = $derived(data.person.id === data.user.id);
	let canEdit = $derived(data.user.grantedPermissions.includes('user:update'));
	let canAssign = $derived(data.user.grantedPermissions.includes('role_assignment:manage'));

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update();
			if (result.type === 'success') {
				toast.push({ title: String(result.data?.message ?? 'Saved'), tone: 'success' });
			} else if (result.type === 'failure') {
				toast.push({
					title: String(result.data?.message ?? 'Something went wrong'),
					tone: 'danger'
				});
			}
		};
	};
</script>

<svelte:head>
	<title>{data.person.displayName} · Users · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/users')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All users
	</a>

	<Card title={data.person.displayName} description={data.person.email}>
		{#snippet actions()}
			{#if data.person.kind === 'service'}
				<Badge tone="info">Service account</Badge>
			{:else}
				<Badge tone={data.person.mfaEnabled ? 'success' : 'warning'}>
					{data.person.mfaEnabled ? '2FA on' : '2FA off'}
				</Badge>
			{/if}
			<Badge tone={data.person.status === 'active' ? 'success' : 'neutral'}>
				{data.person.status}
			</Badge>
		{/snippet}
		{#if canEdit && !isSelf}
			<div class="flex flex-wrap gap-2">
				<form method="POST" action="?/status" use:enhance={notify}>
					{#if data.person.status === 'active'}
						<input type="hidden" name="status" value="disabled" />
						<Button type="submit" variant="danger" size="sm">Disable and sign out</Button>
					{:else}
						<input type="hidden" name="status" value="active" />
						<Button type="submit" size="sm">Enable</Button>
					{/if}
				</form>
				{#if data.person.mfaEnabled && data.person.kind === 'human'}
					<form
						method="POST"
						action="?/resetMfa"
						use:enhance={(event) => {
							if (
								!confirm(
									`Remove ${data.person.displayName}'s two-factor authentication? They'll be signed out and can sign in with just their password until they set it up again.`
								)
							) {
								event.cancel();
								return;
							}
							return notify(event);
						}}
					>
						<Button type="submit" size="sm">Reset two-factor authentication</Button>
					</form>
				{/if}
			</div>
		{:else}
			<p class="text-sm text-fg-muted">
				{isSelf ? 'This is your account.' : 'You can view this user but not change them.'}
			</p>
		{/if}
	</Card>

	<Card title="Teams" description="This user also gets the roles of every team they're in.">
		{#if data.teams.length === 0}
			<p class="text-sm text-fg-muted">Not in any team.</p>
		{:else}
			<div class="flex flex-wrap gap-2">
				{#each data.teams as team (team.id)}
					<a
						href={resolve('/(app)/settings/teams/[id]', { id: team.id })}
						class="rounded-full bg-surface-sunken px-3 py-1 text-sm text-fg hover:underline"
					>
						{team.name}
					</a>
				{/each}
			</div>
		{/if}
	</Card>

	<Card title="Roles" description="What this user can do, and where.">
		{#if data.assignments.length === 0}
			<p class="text-sm text-fg-muted">No roles yet. This user can sign in but sees nothing.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.assignments as a (a.id)}
					<li class="flex items-center justify-between gap-4 py-2.5 first:pt-0">
						<div>
							<p class="text-sm font-medium text-fg">{a.roleName}</p>
							<p class="text-sm text-fg-muted">
								at {a.orgName}{a.inherit ? ' and its sub-organizations' : ' only'}
							</p>
						</div>
						{#if canAssign}
							<form method="POST" action="?/unassign" use:enhance={notify}>
								<input type="hidden" name="id" value={a.id} />
								<Button type="submit" size="sm" variant="ghost">Remove</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}

		{#if canAssign && data.roles.length > 0}
			<form
				method="POST"
				action="?/assign"
				use:enhance={notify}
				class="mt-5 grid gap-4 border-t border-border pt-5 sm:grid-cols-[1fr_1fr_auto] sm:items-end"
			>
				<Field label="Role" for="assign-role">
					<Select
						id="assign-role"
						name="roleId"
						required
						placeholder="Choose a role"
						bind:value={roleId}
						options={data.roles.map((r) => ({ value: r.id, label: r.name }))}
					/>
				</Field>
				<Field label="Organization" for="assign-org">
					<Select
						id="assign-org"
						name="orgId"
						required
						placeholder="Choose an organization"
						bind:value={orgId}
						options={data.orgs.map((o) => ({
							value: o.id,
							label: `${'— '.repeat(o.depth - 1)}${o.name}`
						}))}
					/>
				</Field>
				<Button type="submit" variant="primary">Assign</Button>
				<label class="flex items-center gap-2 text-sm text-fg-muted sm:col-span-3">
					<input type="checkbox" name="inherit" checked class="size-4 accent-accent" />
					Also applies to sub-organizations
				</label>
			</form>
		{/if}
	</Card>
</div>
