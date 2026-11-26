<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		Dialog,
		EmptyState,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Account = components['schemas']['PortalAccount'];
	type Provider = components['schemas']['PortalIdentityProvider'];
	type Role = components['schemas']['PortalRole'];

	type Props = { accounts: Account[]; providers: Provider[]; canManage: boolean };
	let { accounts, providers, canManage }: Props = $props();

	const ROLE: Record<string, string> = {
		client_user: 'Client user',
		client_approver: 'Client approver',
		client_admin: 'Client admin'
	};
	const ROLES = Object.entries(ROLE).map(([value, label]) => ({ value, label }));

	let open = $state(false);
	let editing = $state<Provider | null>(null);
	let name = $state('');
	let preset = $state('entra');
	let tenant = $state('');
	let issuer = $state('');
	let clientId = $state('');
	let secret = $state('');
	let domains = $state('');
	let provision = $state(false);
	let defaultRole = $state<Role>('client_user');
	let enabled = $state(true);

	function edit(p: Provider | null) {
		editing = p;
		name = p?.name ?? '';
		preset = p?.preset ?? 'entra';
		issuer = p?.issuerUrl ?? '';
		tenant = p?.issuerUrl.match(/login\.microsoftonline\.com\/([^/]+)\/v2\.0/)?.[1] ?? '';
		clientId = p?.clientId ?? '';
		secret = '';
		domains = (p?.emailDomains ?? []).join(', ');
		provision = p?.provision ?? false;
		defaultRole = p?.defaultRole ?? 'client_user';
		enabled = p?.enabled ?? true;
		open = true;
	}

	let issuerUrl = $derived(
		preset === 'entra' && tenant.trim()
			? `https://login.microsoftonline.com/${tenant.trim()}/v2.0`
			: issuer.trim()
	);
	let providerJson = $derived(
		JSON.stringify({
			name,
			preset,
			issuerUrl,
			clientId,
			...(secret ? { clientSecret: secret } : {}),
			emailDomains: domains
				.split(/[\s,]+/)
				.map((d) => d.trim())
				.filter(Boolean),
			provision,
			defaultRole,
			enabled,
			...(editing ? { version: editing.version } : {})
		})
	);

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const msg = (result.data as { portal?: { done?: string } } | undefined)?.portal?.done;
				toast.push({ title: msg ?? 'Saved.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { portal?: { message?: string } } | undefined;
				toast.push({ title: failed?.portal?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<div class="flex flex-col gap-6">
	<Card title="Portal access" description="Contacts who can sign in to the client portal.">
		{#if accounts.length === 0}
			<EmptyState
				title="Nobody yet"
				description="Invite contacts from their page, or add a sign-in provider below."
			/>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Portal accounts">
				{#each accounts as a (a.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2.5 first:pt-0">
						<span>
							<a
								href={resolve('/(app)/contacts/[id]', { id: a.contactId })}
								class="font-medium text-fg hover:underline"
							>
								{a.name}
							</a>
							<span class="text-sm text-fg-muted">{a.email}</span>
						</span>
						<span class="flex items-center gap-2">
							<Badge>{ROLE[a.role]}</Badge>
							<Badge
								tone={a.status === 'active'
									? 'success'
									: a.status === 'invited'
										? 'info'
										: 'neutral'}
							>
								{a.status}
							</Badge>
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card
		title="Sign-in with their organization"
		description="Let contacts sign in to the portal with their company's own accounts, such as their Microsoft Entra tenant, matched by email domain."
	>
		{#snippet actions()}
			{#if canManage}
				<Button size="sm" onclick={() => edit(null)}><Plus size={14} /> Add a provider</Button>
			{/if}
		{/snippet}
		{#if providers.length === 0}
			<p class="text-sm text-fg-muted">None: contacts sign in with emailed links or passwords.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Sign-in providers">
				{#each providers as p (p.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2.5 first:pt-0">
						<span>
							{#if canManage}
								<button
									type="button"
									class="font-medium text-fg hover:underline"
									onclick={() => edit(p)}
								>
									{p.name}
								</button>
							{:else}<span class="font-medium text-fg">{p.name}</span>{/if}
							<span class="text-sm text-fg-muted">{p.emailDomains.join(', ')}</span>
						</span>
						<span class="flex items-center gap-2">
							{#if p.provision}<Badge tone="info">Gives access to contacts</Badge>{/if}
							{#if !p.enabled}<Badge tone="neutral">Off</Badge>{/if}
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'Add a sign-in provider'}>
	<form
		method="POST"
		action="?tab=portal&/providerSave"
		class="flex flex-col gap-4"
		use:enhance={done(() => (open = false))}
	>
		{#if editing}<input type="hidden" name="providerId" value={editing.id} />{/if}
		<input type="hidden" name="provider" value={providerJson} />
		<Field
			label="Provider name"
			for="idp-name"
			hint="Shown to contacts, e.g. “Contoso (Microsoft)”"
		>
			<Input id="idp-name" bind:value={name} required maxlength={100} />
		</Field>
		<Field label="Kind" for="idp-preset">
			<Select
				id="idp-preset"
				bind:value={preset}
				options={[
					{ value: 'entra', label: 'Microsoft Entra ID' },
					{ value: 'generic', label: 'Another OpenID Connect provider' }
				]}
			/>
		</Field>
		{#if preset === 'entra'}
			<Field label="Tenant ID" for="idp-tenant" hint="The client's directory (tenant) ID">
				<Input id="idp-tenant" bind:value={tenant} required />
			</Field>
		{:else}
			<Field label="Issuer URL" for="idp-issuer">
				<Input id="idp-issuer" bind:value={issuer} required placeholder="https://" />
			</Field>
		{/if}
		<Field label="Client ID" for="idp-client">
			<Input id="idp-client" bind:value={clientId} required />
		</Field>
		<Field
			label="Client secret"
			for="idp-secret"
			hint={editing?.hasSecret ? 'Set. Leave empty to keep it.' : undefined}
		>
			<Input id="idp-secret" type="password" bind:value={secret} autocomplete="off" />
		</Field>
		<Field label="Email domains" for="idp-domains" hint="Separated by commas, e.g. contoso.com">
			<Input id="idp-domains" bind:value={domains} required />
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" bind:checked={provision} /> Give portal access to contacts with a matching
			address when they first sign in
		</label>
		{#if provision}
			<Field label="Their role" for="idp-role">
				<Select id="idp-role" bind:value={defaultRole} options={ROLES} />
			</Field>
		{/if}
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" bind:checked={enabled} /> On
		</label>
		<div class="flex justify-between gap-2">
			<div>
				{#if editing}<Button
						type="submit"
						variant="danger"
						formaction="?tab=portal&/providerDelete"
					>
						Delete
					</Button>{/if}
			</div>
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</div>
	</form>
</Dialog>
