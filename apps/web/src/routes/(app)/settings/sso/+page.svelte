<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import ProviderFields from './ProviderFields.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let adding = $state(false);
	let errors = $derived(form?.errors ?? {});
</script>

<svelte:head>
	<title>Single sign-on · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Identity providers"
		description="Let people sign in with Microsoft Entra ID, Google, Okta, or another OpenID Connect provider."
	>
		{#snippet actions()}
			{#if !adding && !form?.errors}
				<Button size="sm" variant="primary" onclick={() => (adding = true)}>Add provider</Button>
			{/if}
		{/snippet}
		{#if data.providers.length === 0}
			<p class="text-sm text-fg-muted">No identity providers yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.providers as provider (provider.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/settings/sso/[id]', { id: provider.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{provider.name}
							</a>
							<p class="mt-0.5 text-sm text-fg-muted">
								{provider.allowedDomains.join(', ')} · {provider.issuerUrl}
							</p>
						</div>
						<Badge tone={provider.enabled ? 'success' : 'neutral'}>
							{provider.enabled ? 'Enabled' : 'Disabled'}
						</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if adding || form?.errors}
		<Card
			title="Add an identity provider"
			description="Register this redirect URI with the provider: {data.redirectUri}"
		>
			<form method="POST" action="?/create" use:enhance>
				{#if form?.message && Object.keys(errors).length === 0}
					<p role="alert" class="mb-4 text-sm text-danger">{form.message}</p>
				{/if}
				<ProviderFields {errors} />
				<div class="mt-6 flex gap-2">
					<Button type="submit" variant="primary">Add provider</Button>
					<Button type="button" onclick={() => (adding = false)}>Cancel</Button>
				</div>
			</form>
		</Card>
	{/if}
</div>
