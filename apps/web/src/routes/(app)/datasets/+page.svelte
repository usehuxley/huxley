<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Database } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('dataset:manage'));
	let orgId = $state('');
	$effect(() => {
		if (!orgId) orgId = data.currentOrg?.id ?? data.orgs[0]?.id ?? '';
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	const types = ['text', 'integer', 'number', 'boolean', 'timestamp', 'date', 'json', 'uuid'].map(
		(t) => ({ value: t, label: t })
	);
	let columns = $state([{ name: '', type: 'text', key: true }]);
</script>

<svelte:head>
	<title>Datasets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<h1 class="text-2xl font-semibold text-fg">Datasets</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Typed tables of data kept in Huxley, synced from integrations or written by workflows, for
			workflows and forms to use. Each organization sees only its own rows.
		</p>
	</header>
	<Card title="Datasets">
		{#if data.datasets.length === 0}
			<EmptyState title="No datasets yet" description="Create one below.">
				{#snippet icon()}<Database size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Datasets">
				{#each data.datasets as dataset (dataset.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/datasets/[id]', { id: dataset.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{dataset.name}
							</a>
							<p class="font-mono text-xs text-fg-muted">{dataset.key}</p>
						</div>
						<Badge tone="neutral">{dataset.columns.length} columns</Badge>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
	{#if canManage}
		<Card title="New dataset">
			<form method="POST" action="?/create" use:enhance class="flex flex-col gap-4">
				<div class="flex flex-wrap items-end gap-3">
					<Field label="Name" for="name">
						<Input id="name" name="name" required maxlength={200} />
					</Field>
					<Field label="Organization" for="orgId">
						<Select id="orgId" name="orgId" options={orgOptions} bind:value={orgId} />
					</Field>
				</div>
				<fieldset class="flex flex-col gap-2">
					<legend class="text-sm font-medium text-fg">Columns</legend>
					<p class="text-xs text-fg-muted">
						Lowercase letters, digits and underscores. Key columns identify a row within an
						organization: writing a row with the same key updates it.
					</p>
					{#each columns as column, i (i)}
						<div class="flex flex-wrap items-end gap-3">
							<Field label="Column {i + 1}" for="column-{i}">
								<Input
									id="column-{i}"
									name="columnName"
									bind:value={column.name}
									pattern="[a-z][a-z0-9_]*"
									maxlength={63}
								/>
							</Field>
							<Field label="Type" for="type-{i}">
								<Select id="type-{i}" name="columnType" options={types} bind:value={column.type} />
							</Field>
							<label class="flex items-center gap-1.5 pb-2 text-sm">
								<input type="checkbox" name="columnKey" value={i} bind:checked={column.key} /> Key
							</label>
							{#if columns.length > 1}
								<Button
									type="button"
									variant="ghost"
									size="sm"
									onclick={() => columns.splice(i, 1)}
								>
									Remove
								</Button>
							{/if}
						</div>
					{/each}
					<div>
						<Button
							type="button"
							variant="secondary"
							size="sm"
							onclick={() => columns.push({ name: '', type: 'text', key: false })}
						>
							Add column
						</Button>
					</div>
				</fieldset>
				<div><Button type="submit">Create</Button></div>
			</form>
			{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		</Card>
	{/if}
</div>
