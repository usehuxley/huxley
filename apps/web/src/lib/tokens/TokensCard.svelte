<script lang="ts">
	import { enhance } from '$app/forms';
	import type { components } from '@usehuxley/api-client';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';

	type Token = components['schemas']['ApiTokenView'];
	type Permission = components['schemas']['Permission'];

	let {
		tokens,
		permissions,
		allowNever = false,
		created = null,
		error = null,
		description
	}: {
		tokens: Token[];
		permissions: Permission[];
		/** Offer non-expiring tokens (service accounts). */
		allowNever?: boolean;
		/** The secret of a token just created, shown once. */
		created?: { name: string; secret: string } | null;
		error?: string | null;
		description: string;
	} = $props();

	let scopeMode = $state('all');
	const expiries = $derived([
		{ value: '7', label: '7 days' },
		{ value: '30', label: '30 days' },
		{ value: '90', label: '90 days' },
		{ value: '365', label: '1 year' },
		...(allowNever ? [{ value: 'never', label: 'Never' }] : [])
	]);
	const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });
	const fmt = (iso: string | null | undefined) => (iso ? dateFormat.format(new Date(iso)) : '—');
</script>

<Card title="API tokens" {description}>
	<div class="flex flex-col gap-4">
		{#if error}<p role="alert" class="text-sm text-danger">{error}</p>{/if}
		{#if created}
			<div class="rounded-control border border-warning/40 bg-warning-soft p-4">
				<p class="text-sm font-medium text-fg">Copy “{created.name}” now</p>
				<p class="mt-1 text-sm text-fg-muted">
					It won't be shown again. Send it as <code>Authorization: Bearer …</code>.
				</p>
				<code class="mt-3 block font-mono text-sm break-all text-fg" data-testid="token-secret">
					{created.secret}
				</code>
			</div>
		{/if}
		{#if tokens.length > 0}
			<ul class="flex flex-col divide-y divide-border">
				{#each tokens as token (token.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-medium text-fg">
								{token.name}
								<code class="font-mono text-xs text-fg-muted">{token.hint}…</code>
								{#if token.expired}<Badge tone="danger">Expired</Badge>{/if}
							</p>
							<p class="mt-0.5 text-sm text-fg-muted">
								{token.scopes.includes('*') ? 'All permissions' : token.scopes.join(', ')} · Expires
								{token.expiresAt ? fmt(token.expiresAt) : 'never'} · Last used {fmt(
									token.lastUsedAt
								)}
							</p>
						</div>
						<form method="POST" action="?/revokeToken" use:enhance>
							<input type="hidden" name="id" value={token.id} />
							<Button type="submit" size="sm" aria-label="Revoke {token.name}">Revoke</Button>
						</form>
					</li>
				{/each}
			</ul>
		{/if}
		<form method="POST" action="?/createToken" class="flex flex-col gap-4" use:enhance>
			<div class="flex flex-wrap items-end gap-2">
				<Field label="Token name" for="tokenName">
					<Input
						id="tokenName"
						name="name"
						required
						maxlength={100}
						placeholder="e.g. Reporting script"
					/>
				</Field>
				<Field label="Expires" for="tokenExpiry">
					<Select id="tokenExpiry" name="expiresInDays" value="30" options={expiries} />
				</Field>
				<Field label="Can use" for="scopeMode">
					<Select
						id="scopeMode"
						name="scopeMode"
						bind:value={scopeMode}
						options={[
							{ value: 'all', label: 'All permissions' },
							{ value: 'some', label: 'Only selected permissions' }
						]}
					/>
				</Field>
				<Button type="submit" variant="primary">Create token</Button>
			</div>
			{#if scopeMode === 'some'}
				<fieldset class="grid gap-1 sm:grid-cols-2">
					<legend class="mb-2 text-sm text-fg-muted">
						A token never gets more than its owner's roles allow.
					</legend>
					{#each permissions as permission (permission.key)}
						<label class="flex items-center gap-2 text-sm text-fg">
							<input type="checkbox" name="scopes" value={permission.key} />
							<span>{permission.description}</span>
							<code class="font-mono text-xs text-fg-muted">{permission.key}</code>
						</label>
					{/each}
				</fieldset>
			{/if}
		</form>
	</div>
</Card>
