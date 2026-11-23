<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('team:manage'));
</script>

<svelte:head>
	<title>Teams · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Teams"
		description="Groups of people who share roles. Members get a team's roles on top of their own."
	>
		{#if data.teams.length === 0}
			<p class="text-sm text-fg-muted">No teams yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.teams as team (team.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/settings/teams/[id]', { id: team.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{team.name}
							</a>
							{#if team.description}<p class="mt-0.5 text-sm text-fg-muted">
									{team.description}
								</p>{/if}
						</div>
						<span class="text-sm text-fg-muted">
							{team.memberCount} member{team.memberCount === 1 ? '' : 's'}
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if canManage}
		<Card title="New team">
			<form method="POST" action="?/create" class="flex flex-wrap items-end gap-2" use:enhance>
				{#if form?.message}<p role="alert" class="w-full text-sm text-danger">
						{form.message}
					</p>{/if}
				<Field label="Team name" for="teamName">
					<Input
						id="teamName"
						name="name"
						required
						maxlength={100}
						placeholder="e.g. Service desk"
					/>
				</Field>
				<Field label="Description" for="teamDescription">
					<Input id="teamDescription" name="description" maxlength={500} />
				</Field>
				<Button type="submit" variant="primary">Create team</Button>
			</form>
		</Card>
	{/if}
</div>
