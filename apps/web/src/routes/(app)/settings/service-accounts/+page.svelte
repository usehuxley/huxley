<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Service accounts · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Service accounts"
		description="Identities for integrations and scripts. They hold roles like people but sign in only with API tokens."
	>
		{#if data.accounts.length === 0}
			<p class="text-sm text-fg-muted">No service accounts yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.accounts as account (account.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/settings/service-accounts/[id]', { id: account.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{account.displayName}
							</a>
							{#if account.description}
								<p class="mt-0.5 text-sm text-fg-muted">{account.description}</p>
							{/if}
						</div>
						<Badge tone={account.status === 'active' ? 'success' : 'neutral'}>
							{account.status}
						</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card title="New service account">
		<form method="POST" action="?/create" class="flex flex-wrap items-end gap-2" use:enhance>
			{#if form?.message}<p role="alert" class="w-full text-sm text-danger">{form.message}</p>{/if}
			<Field label="Name" for="saName">
				<Input id="saName" name="name" required maxlength={200} placeholder="e.g. RMM sync" />
			</Field>
			<Field label="Description" for="saDescription">
				<Input id="saDescription" name="description" maxlength={500} />
			</Field>
			<Button type="submit" variant="primary">Create</Button>
		</form>
	</Card>
</div>
