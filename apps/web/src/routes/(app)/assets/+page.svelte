<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { Boxes, Plus, Search } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Input, Select } from '@usehuxley/ui';
	import { STATUSES, statusLabel, statusTone } from '#lib/assets/form.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const withParams = (changes: Record<string, string | number | undefined>) => {
		const params = new SvelteURLSearchParams(page.url.search);
		for (const [key, value] of Object.entries(changes)) {
			if (value === undefined || value === '') params.delete(key);
			else params.set(key, String(value));
		}
		const query = params.toString();
		return `${resolve('/(app)/assets')}${query ? `?${query}` : ''}`;
	};
	let newHref = $derived(
		data.orgId ? `${resolve('/(app)/assets/new')}?org=${data.orgId}` : resolve('/(app)/assets/new')
	);
</script>

<svelte:head>
	<title>Assets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Assets</h1>
			<p class="mt-1 text-fg-muted">
				Your clients' devices, tenants, licenses and domains, and how they depend on each other.
			</p>
		</div>
		{#if data.canWrite}
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved, with ?org= -->
			<Button href={newHref} variant="primary"><Plus size={16} /> New asset</Button>
		{/if}
	</header>

	<form method="GET" class="flex flex-wrap items-end gap-2" role="search">
		<label for="asset-q" class="sr-only">Search assets</label>
		<Input
			id="asset-q"
			name="q"
			value={data.q}
			placeholder="Name, serial, hostname, model"
			class="w-64"
		/>
		<div class="w-44">
			<label for="asset-org-filter" class="sr-only">Organization</label>
			<Select
				id="asset-org-filter"
				name="org"
				value={data.orgId}
				options={[{ value: '', label: 'All organizations' }, ...data.orgs]}
			/>
		</div>
		<div class="w-40">
			<label for="asset-type-filter" class="sr-only">Type</label>
			<Select
				id="asset-type-filter"
				name="type"
				value={data.typeId}
				options={[
					{ value: '', label: 'All types' },
					...data.types.map((t) => ({ value: t.id, label: t.name }))
				]}
			/>
		</div>
		<div class="w-40">
			<label for="asset-status-filter" class="sr-only">Status</label>
			<Select
				id="asset-status-filter"
				name="status"
				value={data.status}
				options={[{ value: '', label: 'In use' }, ...STATUSES]}
			/>
		</div>
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	<Card>
		{#if data.assets.length === 0}
			<EmptyState
				title="No assets match"
				description="Add a client's devices, tenants and licenses, or sync them from an RMM later."
			>
				{#snippet icon()}<Boxes size={20} />{/snippet}
			</EmptyState>
		{:else}
			<table class="w-full text-sm" aria-label="Assets">
				<thead class="text-left text-xs text-fg-muted">
					<tr>
						<th class="py-2 font-medium">Name</th>
						<th class="py-2 font-medium">Type</th>
						<th class="py-2 font-medium">Client</th>
						<th class="py-2 font-medium">Status</th>
						<th class="hidden py-2 font-medium md:table-cell">Serial</th>
						<th class="hidden py-2 font-medium md:table-cell">Used by</th>
					</tr>
				</thead>
				<tbody class="divide-y divide-border">
					{#each data.assets as a (a.id)}
						<tr>
							<td class="py-2">
								<a
									class="font-medium text-fg hover:underline"
									href={resolve('/(app)/assets/[id]', { id: a.id })}
								>
									{a.name}
								</a>
							</td>
							<td class="py-2 text-fg-muted">{a.typeName}</td>
							<td class="py-2 text-fg-muted">{a.orgName}</td>
							<td class="py-2">
								<Badge tone={statusTone(a.status)}>{statusLabel(a.status)}</Badge>
							</td>
							<td class="hidden py-2 font-mono text-xs text-fg-muted md:table-cell">
								{a.serialNumber}
							</td>
							<td class="hidden py-2 text-fg-muted md:table-cell">{a.contactName ?? ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</Card>

	{#if data.pages > 1}
		<div class="flex items-center justify-between text-sm text-fg-muted">
			<span>Page {data.page} of {data.pages} · {data.total} assets</span>
			<div class="flex gap-2">
				<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with a new query -->
				{#if data.page > 1}<Button href={withParams({ page: data.page - 1 })} size="sm">
						Previous
					</Button>{/if}
				{#if data.page < data.pages}<Button href={withParams({ page: data.page + 1 })} size="sm">
						Next
					</Button>{/if}
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			</div>
		</div>
	{/if}
</div>
