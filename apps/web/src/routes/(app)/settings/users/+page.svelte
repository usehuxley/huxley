<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { UserPlus } from '@lucide/svelte';
	import { Badge, Button, Dialog, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let open = $state(false);
	let dialogError = $state('');
	let canCreate = $derived(data.user.grantedPermissions.includes('user:create'));

	const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

	const onCreate: SubmitFunction = () => {
		dialogError = '';
		return async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') open = false;
			else if (result.type === 'failure') {
				dialogError = String(result.data?.message ?? 'Could not create the user.');
			}
		};
	};
	let invite = $state(false);
</script>

<svelte:head>
	<title>Users · Settings · Huxley</title>
</svelte:head>

{#if form && 'created' in form && form.created}
	<div role="status" class="rounded-card border border-success/30 bg-success-soft p-4 text-sm">
		<p class="font-medium text-fg">Created {form.created.email}.</p>
		{#if form.invited}
			<p class="mt-1 text-fg-muted">An invitation to choose a password is on its way.</p>
		{/if}
		{#if form.generatedPassword}
			<p class="mt-1 text-fg-muted">
				Temporary password, shown only once:
				<code class="rounded bg-surface px-1.5 py-0.5 font-mono text-fg">
					{form.generatedPassword}
				</code>
			</p>
		{/if}
		<p class="mt-1 text-fg-muted">
			They have no access until you
			<a
				class="text-accent underline"
				href={resolve('/(app)/settings/users/[id]', { id: form.created.id })}
			>
				assign a role
			</a>.
		</p>
	</div>
{/if}

<section class="rounded-card border border-border bg-surface">
	<header class="flex items-center justify-between border-b border-border px-5 py-3">
		<h2 class="text-sm font-semibold text-fg">People</h2>
		{#if canCreate}
			<Button size="sm" variant="primary" onclick={() => ((open = true), (dialogError = ''))}>
				<UserPlus size={16} /> Add user
			</Button>
		{/if}
	</header>
	<div class="overflow-x-auto">
		<table class="w-full text-left text-sm">
			<thead
				class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
			>
				<tr>
					<th scope="col" class="px-4 py-2.5 font-medium">Name</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Email</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Status</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Last sign-in</th>
				</tr>
			</thead>
			<tbody>
				{#each data.page.items as person (person.id)}
					<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
						<td class="px-4 py-2.5 font-medium">
							<a
								class="text-fg hover:text-accent"
								href={resolve('/(app)/settings/users/[id]', { id: person.id })}
							>
								{person.displayName}
							</a>
						</td>
						<td class="px-4 py-2.5 text-fg-muted">{person.email}</td>
						<td class="px-4 py-2.5">
							<Badge tone={person.status === 'active' ? 'success' : 'neutral'}>
								{person.status}
							</Badge>
						</td>
						<td class="px-4 py-2.5 text-fg-muted">
							{person.lastLoginAt ? dateFormat.format(new Date(person.lastLoginAt)) : 'Never'}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	{#if data.page.nextCursor}
		<footer class="border-t border-border px-5 py-3 text-right">
			<Button size="sm" href="?cursor={encodeURIComponent(data.page.nextCursor)}">Next page</Button>
		</footer>
	{/if}
</section>

<Dialog bind:open title="Add user" description="They can sign in with email and password.">
	<form method="POST" action="?/create" use:enhance={onCreate} class="flex flex-col gap-4">
		{#if dialogError}
			<p
				role="alert"
				class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger"
			>
				{dialogError}
			</p>
		{/if}
		<Field label="Name" for="new-name" required>
			<Input id="new-name" name="displayName" required maxlength={200} />
		</Field>
		<Field label="Email" for="new-email" required>
			<Input id="new-email" name="email" type="email" required autocomplete="off" />
		</Field>
		{#if data.emailEnabled}
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="invite" bind:checked={invite} />
				Email them an invitation to choose their own password
			</label>
		{/if}
		{#if !invite}
			<Field
				label="Password"
				for="new-password"
				hint="Leave blank to generate a temporary password."
			>
				<Input
					id="new-password"
					name="password"
					type="password"
					minlength={12}
					autocomplete="new-password"
				/>
			</Field>
		{/if}
		<div class="flex justify-end gap-2">
			<Button onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Create user</Button>
		</div>
	</form>
</Dialog>
