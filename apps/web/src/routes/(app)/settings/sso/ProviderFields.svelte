<script lang="ts">
	import type { components } from '@usehuxley/api-client';
	import { Field, Input, Select } from '@usehuxley/ui';

	type Provider = components['schemas']['Provider'];

	let { provider, errors = {} }: { provider?: Provider; errors?: Record<string, string> } =
		$props();

	// The form starts from the saved preset; changing it only changes the form.
	const savedPreset = () => provider?.preset ?? 'entra';
	let preset = $state(savedPreset());
	// Entra's issuer is derived from the tenant id; show the tenant id back.
	let tenantFromIssuer = $derived(
		provider?.issuerUrl.match(/microsoftonline\.com\/([^/]+)\/v2\.0$/)?.[1]
	);

	const presets = [
		{ value: 'entra', label: 'Microsoft Entra ID' },
		{ value: 'google', label: 'Google' },
		{ value: 'okta', label: 'Okta' },
		{ value: 'authentik', label: 'Authentik' },
		{ value: 'keycloak', label: 'Keycloak' },
		{ value: 'generic', label: 'Other OpenID Connect provider' }
	];
	const mfaTrust = [
		{ value: 'amr', label: 'When the provider reports MFA (amr claim)' },
		{ value: 'always', label: 'Always: the provider enforces MFA' },
		{ value: 'never', label: 'Never: also require a Huxley second factor' }
	];
</script>

<div class="grid gap-4 sm:grid-cols-2">
	<Field label="Provider" for="preset">
		<Select id="preset" name="preset" bind:value={preset} options={presets} />
	</Field>
	<Field label="Button label" for="name" hint="Shown as “Sign in with …”." error={errors.name}>
		<Input id="name" name="name" value={provider?.name ?? ''} required />
	</Field>
	<Field label="Key" for="key" hint="Lowercase letters, digits, dashes." error={errors.key}>
		<Input id="key" name="key" value={provider?.key ?? ''} required />
	</Field>
	{#if preset === 'entra'}
		<Field label="Directory (tenant) id" for="tenantId" error={errors.tenantId}>
			<Input id="tenantId" name="tenantId" value={tenantFromIssuer ?? ''} required />
		</Field>
	{:else if preset !== 'google'}
		<Field label="Issuer URL" for="issuerUrl" error={errors.issuerUrl}>
			<Input
				id="issuerUrl"
				name="issuerUrl"
				type="url"
				value={provider?.issuerUrl ?? ''}
				placeholder="https://idp.example.com"
				required
			/>
		</Field>
	{/if}
	<Field label="Client (application) id" for="clientId" error={errors.clientId}>
		<Input id="clientId" name="clientId" value={provider?.clientId ?? ''} required />
	</Field>
	<Field
		label="Client secret"
		for="clientSecret"
		hint={provider?.hasClientSecret ? 'Stored. Leave empty to keep it.' : undefined}
	>
		<Input id="clientSecret" name="clientSecret" type="password" autocomplete="off" />
	</Field>
	<Field
		label="Allowed email domains"
		for="allowedDomains"
		hint="Only addresses in these domains can sign in. Separate with commas."
		error={errors.allowedDomains}
	>
		<Input
			id="allowedDomains"
			name="allowedDomains"
			value={provider?.allowedDomains.join(', ') ?? ''}
			placeholder="example.com"
			required
		/>
	</Field>
	<Field
		label="Role claim"
		for="roleClaim"
		hint="Claim whose values map to roles, e.g. roles or groups. Empty: no role mapping."
	>
		<Input
			id="roleClaim"
			name="roleClaim"
			value={provider ? (provider.roleClaim ?? '') : preset === 'entra' ? 'roles' : ''}
		/>
	</Field>
	<Field label="Counts as multi-factor" for="mfaTrust">
		<Select id="mfaTrust" name="mfaTrust" value={provider?.mfaTrust ?? 'amr'} options={mfaTrust} />
	</Field>
</div>
<div class="mt-4 flex flex-col gap-2 text-sm text-fg">
	<label class="flex items-center gap-2">
		<input type="checkbox" name="enabled" checked={provider?.enabled ?? true} /> Enabled
	</label>
	<label class="flex items-center gap-2">
		<input type="checkbox" name="jitProvisioning" checked={provider?.jitProvisioning ?? false} />
		Create a Huxley account on first sign-in
	</label>
	<label class="flex items-center gap-2">
		<input type="checkbox" name="trustEmail" checked={provider?.trustEmail ?? preset === 'entra'} />
		Trust email addresses without an email_verified claim (a single Entra tenant)
	</label>
</div>
