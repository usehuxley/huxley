<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, Field, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Account = components['schemas']['PortalAccount'];
	type Role = components['schemas']['PortalRole'];

	type Props = { account: Account | null; hasEmail: boolean; canManage: boolean };
	let { account, hasEmail, canManage }: Props = $props();

	const ROLES: { value: Role; label: string }[] = [
		{ value: 'client_user', label: 'Client user: their own tickets' },
		{ value: 'client_approver', label: 'Client approver: also approves requests' },
		{ value: 'client_admin', label: 'Client admin: all company tickets, assets and access' }
	];
	const STATUS: Record<string, { label: string; tone: 'success' | 'info' | 'neutral' }> = {
		active: { label: 'Active', tone: 'success' },
		invited: { label: 'Invited', tone: 'info' },
		disabled: { label: 'Disabled', tone: 'neutral' }
	};
	let role = $derived<Role>(account?.role ?? 'client_user');

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const message =
					(result.data as { portal?: { done?: string } } | undefined)?.portal?.done ?? 'Done.';
				toast.push({ title: message, tone: 'success' });
			} else if (result.type === 'failure') {
				const failed = result.data as { portal?: { message?: string } } | undefined;
				toast.push({
					title: failed?.portal?.message ?? 'Something went wrong.',
					tone: 'danger'
				});
			}
		};
</script>

<Card
	title="Portal access"
	description="Whether this contact can sign in to the client portal, and what they see there."
>
	{#snippet actions()}
		{#if account}
			<Badge tone={STATUS[account.status]?.tone ?? 'neutral'}>
				{STATUS[account.status]?.label ?? account.status}
			</Badge>
		{/if}
	{/snippet}
	{#if account}
		<p class="text-sm text-fg-muted">
			Signs in as {account.email}{account.lastSignInAt
				? `; last signed in ${new Date(account.lastSignInAt).toLocaleString()}`
				: '; hasn’t signed in yet'}.
		</p>
	{:else}
		<p class="text-sm text-fg-muted">No portal access yet.</p>
	{/if}
	{#if canManage}
		<form
			method="POST"
			action={account ? '?/portalUpdate' : '?/portalInvite'}
			class="mt-3 flex flex-wrap items-end gap-3"
			use:enhance={done}
		>
			{#if account}
				<input type="hidden" name="accountId" value={account.id} />
				<input type="hidden" name="version" value={account.version} />
				<input
					type="hidden"
					name="status"
					value={account.status === 'disabled' ? 'invited' : account.status}
				/>
			{/if}
			<div class="min-w-72 flex-1">
				<Field label="Portal role" for="portal-role">
					<Select id="portal-role" name="role" bind:value={role} options={ROLES} />
				</Field>
			</div>
			{#if !account}
				<Button type="submit" disabled={!hasEmail}>Invite to the portal</Button>
			{:else if account.status === 'disabled'}
				<Button type="submit">Re-enable</Button>
			{:else}
				<Button type="submit" variant="secondary">Save role</Button>
			{/if}
		</form>
		{#if !account && !hasEmail}
			<p class="mt-2 text-sm text-warning">Add an email address first.</p>
		{/if}
		{#if account && account.status !== 'disabled'}
			<div class="mt-3 flex flex-wrap gap-2">
				<form method="POST" action="?/portalResend" use:enhance={done}>
					<input type="hidden" name="accountId" value={account.id} />
					<Button type="submit" size="sm" variant="ghost">Email a new sign-in link</Button>
				</form>
				<form method="POST" action="?/portalUpdate" use:enhance={done}>
					<input type="hidden" name="accountId" value={account.id} />
					<input type="hidden" name="version" value={account.version} />
					<input type="hidden" name="role" value={account.role} />
					<input type="hidden" name="status" value="disabled" />
					<Button type="submit" size="sm" variant="ghost">Disable portal access</Button>
				</form>
			</div>
		{/if}
	{/if}
</Card>
