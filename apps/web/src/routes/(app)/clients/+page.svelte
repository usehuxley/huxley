<script lang="ts">
	import { resolve } from '$app/paths';
	import { Search } from '@lucide/svelte';
	import { Badge, Button, EmptyState, Input } from '@usehuxley/ui';
	import { RELATIONSHIPS, STATUSES, label, statusTone } from '#lib/crm/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Clients · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Clients</h1>
			<p class="mt-1 text-fg-muted">Your clients, with their contacts and sites.</p>
		</div>
		<form method="GET" class="flex gap-2" role="search">
			<label for="q" class="sr-only">Search clients</label>
			<Input id="q" name="q" value={data.search} placeholder="Search by name" class="w-64" />
			<Button type="submit"><Search size={16} /> Search</Button>
		</form>
	</header>

	{#if data.companies.length === 0}
		<EmptyState
			title={data.search ? 'No clients match' : 'No clients yet'}
			description={data.search
				? 'Try another name.'
				: 'Clients are the client organizations you can see. Add them under Settings → Organizations.'}
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Clients">
				<thead
					class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
				>
					<tr>
						<th scope="col" class="px-4 py-2.5 font-medium">Name</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Status</th>
						<th scope="col" class="px-4 py-2.5 font-medium">Relationship</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Contacts</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Sites</th>
					</tr>
				</thead>
				<tbody>
					{#each data.companies as company (company.orgId)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5 font-medium">
								<a
									class="text-fg hover:text-accent"
									href={resolve('/(app)/clients/[id]', { id: company.orgId })}
								>
									{company.name}
								</a>
							</td>
							<td class="px-4 py-2.5">
								<Badge tone={statusTone(company.status)}>{label(STATUSES, company.status)}</Badge>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">
								{label(RELATIONSHIPS, company.relationship)}
							</td>
							<td class="px-4 py-2.5 text-right text-fg-muted tabular-nums">{company.contacts}</td>
							<td class="px-4 py-2.5 text-right text-fg-muted tabular-nums">{company.sites}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>
