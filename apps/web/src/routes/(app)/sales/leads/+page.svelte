<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus, Search } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { LEAD_STATUSES, label, leadTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let adding = $state(false);
	let errors = $derived(form?.errors ?? {});
	$effect(() => {
		if (form?.message) adding = true;
	});
	const person = (l: { firstName?: string | null; lastName?: string | null }) =>
		[l.firstName, l.lastName].filter(Boolean).join(' ');
</script>

<svelte:head>
	<title>Leads · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<a href={resolve('/(app)/sales')} class="text-sm text-fg-muted hover:text-fg">← Pipeline</a>
			<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">Leads</h1>
			<p class="mt-1 text-sm text-fg-subtle">Who may buy, before there's a deal.</p>
		</div>
		{#if data.canWrite}
			<div class="ml-auto">
				<Button variant="primary" onclick={() => (adding = true)}>
					<Plus size={16} /> New lead
				</Button>
			</div>
		{/if}
	</div>

	<form method="GET" class="flex flex-wrap items-center gap-2" role="search">
		<label for="status" class="sr-only">Status</label>
		<Select
			id="status"
			name="status"
			value={data.status}
			options={[{ value: '', label: 'Any status' }, ...LEAD_STATUSES.map((s) => ({ ...s }))]}
		/>
		<label for="q" class="sr-only">Search leads</label>
		<Input id="q" name="q" value={data.q} placeholder="Company, person or email" class="w-64" />
		<Button type="submit"><Search size={16} /> Search</Button>
	</form>

	{#if data.leads.length === 0}
		<EmptyState
			title="No leads"
			description="Leads you capture, or that arrive from forms and the API, show here."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="Leads">
				<thead class="border-b border-border text-xs font-bold text-fg-subtle">
					<tr>
						<th scope="col" class="px-4 py-2.5">Company</th>
						<th scope="col" class="px-4 py-2.5">Person</th>
						<th scope="col" class="px-4 py-2.5">Source</th>
						<th scope="col" class="px-4 py-2.5">Status</th>
						<th scope="col" class="px-4 py-2.5">Owner</th>
						<th scope="col" class="px-4 py-2.5 text-right">Added</th>
					</tr>
				</thead>
				<tbody>
					{#each data.leads as l (l.id)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/sales/leads/[id]', { id: l.id })}
									class="font-semibold text-fg hover:text-accent"
								>
									{l.companyName}
								</a>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">
								{person(l)}{#if l.email}<span class="block text-xs text-fg-subtle">
										{l.email}
									</span>{/if}
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{l.source ?? ''}</td>
							<td class="px-4 py-2.5">
								<Badge tone={leadTone(l.status)}>{label(LEAD_STATUSES, l.status)}</Badge>
							</td>
							<td class="px-4 py-2.5 text-fg-muted">{l.ownerName ?? ''}</td>
							<td class="px-4 py-2.5 text-right whitespace-nowrap text-fg-muted">
								{new Date(l.createdAt).toLocaleDateString()}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<Dialog bind:open={adding} title="New lead">
	<form id="lead-form" method="POST" action="?/create" class="flex flex-col gap-4" use:enhance>
		{#if data.providers.length > 1}
			<Field label="Captured by" for="orgId">
				<Select id="orgId" name="orgId" options={data.providers} />
			</Field>
		{:else}
			<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
		{/if}
		<Field label="Company" for="companyName" error={errors.companyName}>
			<Input id="companyName" name="companyName" required maxlength={200} />
		</Field>
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="First name" for="firstName">
				<Input id="firstName" name="firstName" maxlength={100} />
			</Field>
			<Field label="Last name" for="lastName">
				<Input id="lastName" name="lastName" maxlength={100} />
			</Field>
		</div>
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Email" for="email" error={errors.email}>
				<Input id="email" name="email" type="email" />
			</Field>
			<Field label="Phone" for="phone"><Input id="phone" name="phone" maxlength={50} /></Field>
		</div>
		<Field label="Source" for="source">
			<Input id="source" name="source" maxlength={60} placeholder="Referral, web, event…" />
		</Field>
		{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (adding = false)}>Cancel</Button>
		<Button type="submit" form="lead-form" variant="primary">Add lead</Button>
	{/snippet}
</Dialog>
