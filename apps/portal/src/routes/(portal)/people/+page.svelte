<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let giveAccess = $state(true);

	const ROLES = [
		{ value: 'client_user', label: 'Their own requests' },
		{ value: 'client_approver', label: 'Also approves' },
		{ value: 'client_admin', label: 'Admin: everything, and people' }
	];
	const STATUS: Record<string, string> = {
		invited: 'Invited',
		active: 'Has access',
		disabled: 'No longer has access'
	};
</script>

<svelte:head>
	<title>People · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">People</h1>
		<p class="mt-1 text-fg-muted">Who at {data.me.orgName} can use this portal.</p>
	</div>
	{#if form && 'done' in form}
		<p class="rounded-card border border-border bg-surface p-3 text-sm text-fg" role="status">
			{form.done}
		</p>
	{/if}
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-danger" role="alert">{form.message}</p>
	{/if}

	<Card>
		<ul class="flex flex-col divide-y divide-border" aria-label="People">
			{#each data.people as p (p.contactId)}
				{@const me = p.contactId === data.me.contactId}
				<li
					class="flex flex-col gap-2 py-3 first:pt-0 sm:flex-row sm:items-center sm:justify-between"
				>
					<div class="min-w-0">
						<span class="font-medium text-fg">{p.name}</span>
						{#if me}<Badge>You</Badge>{/if}
						<p class="text-sm text-fg-muted">{p.email ?? 'No email address'}</p>
					</div>
					{#if me}
						<Badge tone="success">{STATUS[p.status ?? 'active']}</Badge>
					{:else if !p.accountId || !p.role}
						{#if p.email}
							<form method="POST" action="?/give" class="flex items-center gap-2" use:enhance>
								<input type="hidden" name="contact" value={p.contactId} />
								<input type="hidden" name="name" value={p.name} />
								<div class="w-52">
									<Select
										name="role"
										aria-label="Access for {p.name}"
										options={ROLES}
										value="client_user"
									/>
								</div>
								<Button type="submit" size="sm">Give access</Button>
							</form>
						{:else}
							<span class="text-sm text-fg-subtle">Needs an email address</span>
						{/if}
					{:else}
						<form
							method="POST"
							action="?/change"
							class="flex flex-wrap items-center gap-2"
							use:enhance
						>
							<input type="hidden" name="contact" value={p.contactId} />
							<input type="hidden" name="version" value={p.version} />
							<Badge
								tone={p.status === 'disabled'
									? 'neutral'
									: p.status === 'invited'
										? 'info'
										: 'success'}
							>
								{STATUS[p.status ?? 'invited']}
							</Badge>
							<div class="w-52">
								<Select
									name="role"
									aria-label="Access for {p.name}"
									options={ROLES}
									value={p.role}
								/>
							</div>
							{#if p.status === 'disabled'}
								<Button type="submit" size="sm" name="status" value="active">Restore access</Button>
							{:else}
								<Button type="submit" size="sm" name="status" value={p.status}>Save</Button>
								<Button type="submit" size="sm" variant="ghost" name="status" value="disabled">
									Remove access
								</Button>
							{/if}
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	</Card>

	<Card title="Add someone">
		<form method="POST" action="?/add" class="flex flex-col gap-4" use:enhance>
			<div class="grid gap-4 sm:grid-cols-3">
				<Field label="First name" for="p-first">
					<Input
						id="p-first"
						name="firstName"
						required
						value={form && 'firstName' in form ? form.firstName : ''}
					/>
				</Field>
				<Field label="Last name" for="p-last">
					<Input
						id="p-last"
						name="lastName"
						value={form && 'lastName' in form ? form.lastName : ''}
					/>
				</Field>
				<Field label="Work email" for="p-email">
					<Input
						id="p-email"
						name="email"
						type="email"
						required
						value={form && 'email' in form ? form.email : ''}
					/>
				</Field>
			</div>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="access" bind:checked={giveAccess} /> Give them access to this portal
			</label>
			{#if giveAccess}
				<div class="w-64">
					<Select name="role" aria-label="Their access" options={ROLES} value="client_user" />
				</div>
			{/if}
			<div><Button type="submit" variant="primary">Add</Button></div>
		</form>
	</Card>
</div>
