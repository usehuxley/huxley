<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { ClipboardList } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let granted = $derived(data.user.grantedPermissions);
	let canManage = $derived(granted.includes('form:manage'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
</script>

<svelte:head>
	<title>Forms · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Forms</h1>
		<p class="mt-1 text-sm text-fg-muted">Fill one in to start the workflows that listen for it.</p>
	</header>
	<Card title="All forms">
		{#if data.forms.length === 0}
			<EmptyState title="No forms yet" description="Forms you can fill in show up here.">
				{#snippet icon()}<ClipboardList size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Forms">
				{#each data.forms as f (f.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/forms/[id]', { id: f.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{f.name}
							</a>
							{#if f.description}<p class="text-xs text-fg-muted">{f.description}</p>{/if}
						</div>
						<div class="flex items-center gap-2">
							{#if f.archived}<Badge tone="neutral">Archived</Badge>{/if}
							{#if canManage}
								<a
									href={resolve('/(app)/forms/[id]/edit', { id: f.id })}
									class="text-sm text-accent hover:underline"
								>
									Edit
								</a>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
	{#if canManage}
		<Card title="New form">
			<form method="POST" action="?/create" use:enhance class="flex flex-wrap items-end gap-3">
				<Field label="Name" for="name">
					<Input id="name" name="name" required maxlength={200} />
				</Field>
				<Field label="Organization" for="orgId">
					<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
				</Field>
				<Button type="submit">Create</Button>
			</form>
			{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		</Card>
	{/if}
</div>
