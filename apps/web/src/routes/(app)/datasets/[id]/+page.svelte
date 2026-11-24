<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Database } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('dataset:manage'));
	let columns = $derived(data.dataset.columns);
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	let pages = $derived(Math.max(1, Math.ceil(data.rows.total / data.pageSize)));
	const when = (v: string | null | undefined) => (v ? new Date(v).toLocaleString() : 'never');
	const cell = (v: unknown): string => {
		if (v === null || v === undefined) return '';
		if (typeof v === 'string') return v;
		if (typeof v === 'number' || typeof v === 'boolean') return String(v);
		return JSON.stringify(v);
	};
	const example = $derived(
		JSON.stringify(
			{
				action: 'msgraph/listUsers',
				map: Object.fromEntries(columns.map((c) => [c.name, c.name])),
				mode: 'replace',
				orgs: 'own',
				schedule: { cron: '0 2 * * *' }
			},
			null,
			2
		)
	);
	let syncText = $derived(data.sync?.sync ? JSON.stringify(data.sync.sync, null, 2) : '');
	let rowsText = $state('');
	let addOrg = $state('');
	$effect(() => {
		if (!addOrg) addOrg = data.dataset.orgId;
	});
	let orgOptions = $derived(data.orgs.map((o) => ({ value: o.id, label: o.name })));
	const pageHref = (page: number) => {
		const query = [
			['column', data.filter.column],
			['value', data.filter.value],
			['org', data.filter.org],
			['page', String(page)]
		]
			.filter(([, v]) => v)
			.map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
			.join('&');
		return `${resolve('/(app)/datasets/[id]', { id: data.dataset.id })}?${query}`;
	};
</script>

<svelte:head>
	<title>{data.dataset.name} · Datasets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header>
		<a href={resolve('/(app)/datasets')} class="text-sm text-fg-muted hover:underline">Datasets</a>
		<h1 class="text-2xl font-semibold text-fg">{data.dataset.name}</h1>
		<p class="mt-1 flex flex-wrap items-center gap-2 text-sm text-fg-muted">
			<span class="font-mono text-xs">{data.dataset.key}</span>
			{#each columns as column (column.name)}
				<Badge tone={data.dataset.primaryKey.includes(column.name) ? 'accent' : 'neutral'}>
					{column.name}: {column.type}
				</Badge>
			{/each}
		</p>
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg" role="status">{form.message}</p>
	{/if}

	<Card title="Rows" description="{data.rows.total} in all (those of organizations you can see).">
		<form method="GET" class="mb-4 flex flex-wrap items-end gap-3">
			<Field label="Column" for="f-column">
				<Select
					id="f-column"
					name="column"
					options={[
						{ value: '', label: 'Any' },
						...columns.map((c) => ({ value: c.name, label: c.name }))
					]}
					value={data.filter.column}
				/>
			</Field>
			<Field label="Equals" for="f-value">
				<Input id="f-value" name="value" value={data.filter.value} />
			</Field>
			<Field label="Organization" for="f-org">
				<Select
					id="f-org"
					name="org"
					options={[{ value: '', label: 'All' }, ...orgOptions]}
					value={data.filter.org}
				/>
			</Field>
			<Button type="submit" variant="secondary">Filter</Button>
		</form>
		{#if data.rowsError}
			<p class="text-sm text-danger" role="alert">{data.rowsError}</p>
		{:else if data.rows.rows.length === 0}
			<EmptyState title="No rows" description="Sync the dataset, or write rows from a workflow.">
				{#snippet icon()}<Database size={20} />{/snippet}
			</EmptyState>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-left text-sm" aria-label="Dataset rows">
					<thead class="text-xs text-fg-muted">
						<tr>
							{#each columns as column (column.name)}<th class="px-2 py-1 font-medium">
									{column.name}
								</th>{/each}
							<th class="px-2 py-1 font-medium">Organization</th>
							<th class="px-2 py-1 font-medium">Synced</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each data.rows.rows as row, i (i)}
							<tr>
								{#each columns as column (column.name)}
									<td class="max-w-xs truncate px-2 py-1">{cell(row[column.name])}</td>
								{/each}
								<td class="px-2 py-1 text-fg-muted">
									{orgName.get(String(row._org_id)) ?? cell(row._org_id)}
								</td>
								<td class="px-2 py-1 text-fg-muted">{when(cell(row._synced_at))}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			{#if pages > 1}
				<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
				<nav class="mt-3 flex items-center gap-3 text-sm" aria-label="Pages">
					{#if data.page > 0}<a href={pageHref(data.page - 1)} class="hover:underline">
							Previous
						</a>{/if}
					<span class="text-fg-muted">Page {data.page + 1} of {pages}</span>
					{#if data.page + 1 < pages}<a href={pageHref(data.page + 1)} class="hover:underline">
							Next
						</a>{/if}
				</nav>
				<!-- eslint-enable svelte/no-navigation-without-resolve -->
			{/if}
		{/if}
	</Card>

	<Card
		title="Sync"
		description="Keep the dataset up to date from an integration's list action, on a schedule, for each organization. Each sync is a run you can follow under Runs."
	>
		<div class="flex flex-col gap-3 text-sm">
			<p class="text-fg-muted">
				Next: {when(data.sync?.nextAt)} · last: {when(data.sync?.lastAt)}
			</p>
			{#if canManage}
				<form
					method="POST"
					action="?/saveSync"
					use:enhance={() =>
						({ update }) =>
							update({ reset: false })}
					class="flex flex-col gap-2"
				>
					<label for="sync" class="font-medium text-fg">
						Sync settings (JSON; empty stops syncing)
					</label>
					<textarea
						id="sync"
						name="sync"
						bind:value={syncText}
						rows={12}
						placeholder={example}
						class="rounded-control border border-border bg-surface p-2 font-mono text-xs"
					></textarea>
					<p class="text-xs text-fg-muted">
						action, map (column → path in each item), mode (replace | upsert), orgs (own | clients |
						all), schedule (&#123; cron | interval, timezone &#125;), and optionally connection,
						with, maxPages. See docs/datasets.md.
					</p>
					<div class="flex gap-2">
						<Button type="submit" variant="secondary">Save sync</Button>
					</div>
				</form>
				{#if data.sync?.sync}
					<form method="POST" action="?/runSync" use:enhance>
						<Button type="submit">Sync now</Button>
					</form>
				{/if}
			{:else if data.sync?.sync}
				<pre
					class="overflow-x-auto rounded-control bg-surface-sunken p-2 font-mono text-xs">{JSON.stringify(
						data.sync.sync,
						null,
						2
					)}</pre>
			{:else}
				<p class="text-fg-muted">This dataset doesn't sync.</p>
			{/if}
		</div>
	</Card>

	{#if canManage}
		<Card title="Add rows" description="Rows with an existing key are updated.">
			<form method="POST" action="?/addRows" use:enhance class="flex flex-col gap-3">
				<Field label="Organization" for="add-org">
					<Select id="add-org" name="orgId" options={orgOptions} bind:value={addOrg} />
				</Field>
				<label for="rows" class="text-sm font-medium text-fg">Rows (a JSON object or list)</label>
				<textarea
					id="rows"
					name="rows"
					bind:value={rowsText}
					rows={5}
					placeholder={JSON.stringify([Object.fromEntries(columns.map((c) => [c.name, '…']))])}
					class="rounded-control border border-border bg-surface p-2 font-mono text-xs"></textarea>
				<div><Button type="submit" variant="secondary">Write rows</Button></div>
			</form>
		</Card>
	{/if}
</div>
