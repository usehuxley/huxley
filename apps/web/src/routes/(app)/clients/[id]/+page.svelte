<script lang="ts">
	import ActionsCard from '#lib/forms/ActionsCard.svelte';
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Globe, MapPin, Phone, Plus, Search, UserPlus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, Tabs, toast } from '@usehuxley/ui';
	import VaultPanel from '#lib/vault/VaultPanel.svelte';
	import AgreementsPanel from '#lib/agreements/AgreementsPanel.svelte';
	import SiteForm from '#lib/crm/SiteForm.svelte';
	import { RELATIONSHIPS, STATUSES, hoursSummary, label, statusTone } from '#lib/crm/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let canManage = $derived(data.user.grantedPermissions.includes('crm:manage'));
	// The tab from the address, at first; then wherever you go.
	const initialTab = () => data.tab;
	let tab = $state(initialTab());
	let company = $derived(data.company);
	let primarySite = $derived(data.sites.find((s) => s.isPrimary) ?? null);

	let relationship = $state('');
	let status = $state('');
	$effect(() => {
		relationship = data.company.relationship;
		status = data.company.status;
	});

	let addingContact = $state(false);
	/** The site being edited ('new' for a new one), in a dialog. */
	let editing = $state<string | null>(null);
	let siteOpen = $state(false);
	const editSite = (id: string) => {
		editing = id;
		siteOpen = true;
	};
	let editingSite = $derived(data.sites.find((s) => s.id === editing) ?? null);

	const companyErrors = $derived(
		form && 'company' in form && form.company && 'errors' in form.company
			? (form.company.errors ?? {})
			: {}
	);
	const siteErrors = $derived(
		form && 'site' in form && form.site && 'errors' in form.site ? (form.site.errors ?? {}) : {}
	);
	const contactErrors = $derived(
		form && 'contact' in form && form.contact && 'errors' in form.contact
			? (form.contact.errors ?? {})
			: {}
	);

	const saved =
		(done: string, close?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: done, tone: 'success' });
				close?.();
			} else if (result.type === 'failure') {
				const data = result.data as Record<string, { message?: string }> | undefined;
				const message = Object.values(data ?? {})[0]?.message;
				if (message) toast.push({ title: message, tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>{data.org.name} · Clients · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/clients')} class="text-sm text-fg-muted hover:text-fg">← Clients</a>
		<div class="mt-2 flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-fg">{data.org.name}</h1>
			<Badge tone={statusTone(company.status)}>{label(STATUSES, company.status)}</Badge>
			<Badge>{label(RELATIONSHIPS, company.relationship)}</Badge>
		</div>
		<p class="mt-2 flex flex-wrap gap-x-5 gap-y-1 text-sm text-fg-muted">
			{#if company.website}
				<span class="flex items-center gap-1.5"><Globe size={14} /> {company.website}</span>
			{/if}
			{#if company.phone}
				<span class="flex items-center gap-1.5"><Phone size={14} /> {company.phone}</span>
			{/if}
			{#if primarySite}
				<span class="flex items-center gap-1.5">
					<MapPin size={14} />
					{[primarySite.name, primarySite.city, primarySite.country].filter(Boolean).join(', ')}
				</span>
			{/if}
		</p>
	</div>

	<Tabs
		bind:value={tab}
		label="Client sections"
		items={[
			{ value: 'overview', label: 'Overview' },
			{ value: 'contacts', label: `Contacts (${data.contacts.total})` },
			{ value: 'sites', label: `Sites (${data.sites.length})` },
			...(data.assets ? [{ value: 'assets', label: `Assets (${data.assets.length})` }] : []),
			...(data.documents
				? [{ value: 'documentation', label: `Documentation (${data.documents.length})` }]
				: []),
			...(data.vault ? [{ value: 'vault', label: `Passwords (${data.vault.length})` }] : []),
			...(data.agreements
				? [{ value: 'agreements', label: `Agreements (${data.agreements.length})` }]
				: [])
		]}
	>
		{#snippet panel(value)}
			{#if value === 'overview'}
				<ActionsCard actions={data.actions} orgId={data.org.id} />
				<Card title="Company details">
					<form
						method="POST"
						action="?/company"
						class="grid gap-4 sm:grid-cols-2"
						use:enhance={saved('Details saved.')}
					>
						<input type="hidden" name="version" value={company.version} />
						<Field label="Status" for="status">
							<Select
								id="status"
								name="status"
								bind:value={status}
								options={STATUSES}
								disabled={!canManage}
							/>
						</Field>
						<Field label="Relationship" for="relationship">
							<Select
								id="relationship"
								name="relationship"
								bind:value={relationship}
								options={RELATIONSHIPS}
								disabled={!canManage}
							/>
						</Field>
						<Field label="Website" for="website" error={companyErrors.website}>
							<Input
								id="website"
								name="website"
								type="url"
								value={company.website ?? ''}
								placeholder="https://"
								disabled={!canManage}
							/>
						</Field>
						<Field label="Phone" for="company-phone" error={companyErrors.phone}>
							<Input
								id="company-phone"
								name="phone"
								type="tel"
								value={company.phone ?? ''}
								disabled={!canManage}
							/>
						</Field>
						<div class="sm:col-span-2">
							<Field label="Notes" for="notes" error={companyErrors.notes}>
								<textarea
									id="notes"
									name="notes"
									rows={4}
									maxlength={10000}
									disabled={!canManage}
									class="rounded-control border border-border-strong bg-surface p-2 text-sm text-fg"
									>{company.notes ?? ''}</textarea
								>
							</Field>
						</div>
						{#if canManage}
							<div><Button type="submit" variant="primary">Save details</Button></div>
						{/if}
					</form>
				</Card>
			{:else if value === 'contacts'}
				<div class="flex flex-col gap-4">
					<div class="flex flex-wrap items-center justify-between gap-3">
						<form method="GET" class="flex gap-2" role="search">
							<input type="hidden" name="tab" value="contacts" />
							<label for="contact-search" class="sr-only">Search contacts</label>
							<Input
								id="contact-search"
								name="q"
								value={data.search}
								placeholder="Name or email"
								class="w-64"
							/>
							<Button type="submit"><Search size={16} /> Search</Button>
						</form>
						{#if canManage}
							<Button variant="primary" onclick={() => (addingContact = true)}>
								<UserPlus size={16} /> Add contact
							</Button>
						{/if}
					</div>
					{#if data.contacts.items.length === 0}
						<p class="text-sm text-fg-muted">
							{data.search ? 'No contacts match.' : 'No contacts yet.'}
						</p>
					{:else}
						<div class="overflow-x-auto rounded-card border border-border bg-surface">
							<table class="w-full text-left text-sm" aria-label="Contacts">
								<thead
									class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
								>
									<tr>
										<th scope="col" class="px-4 py-2.5 font-medium">Name</th>
										<th scope="col" class="px-4 py-2.5 font-medium">Job title</th>
										<th scope="col" class="px-4 py-2.5 font-medium">Email</th>
										<th scope="col" class="px-4 py-2.5 font-medium">Phone</th>
									</tr>
								</thead>
								<tbody>
									{#each data.contacts.items as contact (contact.id)}
										{@const here = contact.companies.find((c) => c.orgId === data.org.id)}
										<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
											<td class="px-4 py-2.5 font-medium">
												<a
													class="text-fg hover:text-accent"
													href={resolve('/(app)/contacts/[id]', { id: contact.id })}
												>
													{contact.name}
												</a>
												{#if here && !here.isPrimary}
													<Badge>Also at {contact.companies[0]?.orgName}</Badge>
												{/if}
											</td>
											<td class="px-4 py-2.5 text-fg-muted">{here?.jobTitle ?? ''}</td>
											<td class="px-4 py-2.5 text-fg-muted">{contact.email ?? ''}</td>
											<td class="px-4 py-2.5 text-fg-muted">
												{contact.phone ?? contact.mobile ?? ''}
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				</div>
			{:else}
				<div class="flex flex-col gap-4">
					{#if canManage}
						<div>
							<Button variant="primary" onclick={() => editSite('new')}>
								<Plus size={16} /> Add site
							</Button>
						</div>
					{/if}
					{#if data.sites.length === 0}
						<p class="text-sm text-fg-muted">No sites yet.</p>
					{:else}
						<ul class="grid gap-3 sm:grid-cols-2" aria-label="Sites">
							{#each data.sites as site (site.id)}
								<li class="flex flex-col gap-2 rounded-card border border-border bg-surface p-4">
									<div class="flex items-start justify-between gap-2">
										<p class="font-medium text-fg">{site.name}</p>
										{#if site.isPrimary}<Badge tone="accent">Primary</Badge>{/if}
									</div>
									<p class="text-sm whitespace-pre-line text-fg-muted">
										{[
											site.addressLine1,
											site.addressLine2,
											[site.postalCode, site.city].filter(Boolean).join(' '),
											[site.region, site.country].filter(Boolean).join(', ')
										]
											.filter(Boolean)
											.join('\n') || 'No address'}
									</p>
									<p class="text-sm text-fg-muted">
										{site.timezone} · {hoursSummary(site.hours)}
									</p>
									{#if canManage}
										<div class="mt-1 flex gap-2">
											<Button
												size="sm"
												onclick={() => editSite(site.id)}
												aria-label="Edit {site.name}"
											>
												Edit
											</Button>
											<form
												method="POST"
												action="?/archiveSite"
												use:enhance={saved('Site archived.')}
											>
												<input type="hidden" name="id" value={site.id} />
												<Button
													size="sm"
													variant="ghost"
													type="submit"
													aria-label="Archive {site.name}"
												>
													Archive
												</Button>
											</form>
										</div>
									{/if}
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			{/if}
			{#if value === 'assets' && data.assets}
				<Card title="Assets">
					{#snippet actions()}
						{#if data.canWriteAssets}
							<Button href={`${resolve('/(app)/assets/new')}?org=${data.org.id}`}>
								<Plus size={16} /> New asset
							</Button>
						{/if}
					{/snippet}
					{#if data.assets.length === 0}
						<p class="text-sm text-fg-muted">No assets yet.</p>
					{:else}
						<ul class="flex flex-col divide-y divide-border" aria-label="Client assets">
							{#each data.assets as asset (asset.id)}
								<li class="flex items-center justify-between gap-2 py-2 first:pt-0">
									<span>
										<a
											class="text-fg hover:underline"
											href={resolve('/(app)/assets/[id]', { id: asset.id })}
										>
											{asset.name}
										</a>
										<span class="text-xs text-fg-subtle">{asset.typeName}</span>
									</span>
									<span class="text-xs text-fg-muted">
										{asset.status}{asset.contactName ? ` · ${asset.contactName}` : ''}
									</span>
								</li>
							{/each}
						</ul>
					{/if}
				</Card>
			{/if}
			{#if value === 'documentation' && data.documents}
				<Card
					title="Documentation"
					description="This client's knowledge base articles: how their systems are set up and run."
				>
					{#snippet actions()}
						{#if data.canWriteDocs}
							<Button href={`${resolve('/(app)/knowledge/new')}?org=${data.org.id}`}>
								<Plus size={16} /> New document
							</Button>
						{/if}
					{/snippet}
					{#if data.documents.length === 0}
						<p class="text-sm text-fg-muted">No documentation yet.</p>
					{:else}
						<ul class="flex flex-col divide-y divide-border" aria-label="Documentation">
							{#each data.documents as doc (doc.id)}
								<li class="flex items-center justify-between gap-2 py-2 first:pt-0">
									<a
										class="text-fg hover:underline"
										href={resolve('/(app)/knowledge/[id]', { id: doc.id })}
									>
										{doc.title}
									</a>
									<span class="text-xs text-fg-subtle">
										{doc.categoryName ?? ''}{doc.status !== 'published'
											? ` · ${doc.status === 'in_review' ? 'In review' : doc.status === 'archived' ? 'Archived' : 'Draft'}`
											: ''}
									</span>
								</li>
							{/each}
						</ul>
					{/if}
				</Card>
			{/if}
			{#if value === 'agreements' && data.agreements}
				<AgreementsPanel
					agreements={data.agreements}
					usage={data.agreementUsage}
					workTypes={data.workTypes}
					assetTypes={data.assetTypes}
					products={data.products}
					canManage={data.canManageAgreements}
					errors={form && 'agreement' in form && form.agreement && 'errors' in form.agreement
						? (form.agreement.errors ?? {})
						: {}}
				/>
			{/if}
			{#if value === 'vault' && data.vault}
				<VaultPanel items={data.vault} canManage={data.canManageVault} />
			{/if}
		{/snippet}
	</Tabs>
</div>

<Dialog
	bind:open={addingContact}
	title="Add contact"
	description="Their primary company is {data.org.name}."
>
	<form
		id="add-contact"
		method="POST"
		action="?/contact"
		class="grid gap-3 sm:grid-cols-2"
		use:enhance={saved('Contact added.', () => (addingContact = false))}
	>
		<Field label="First name" for="first" error={contactErrors.firstName}>
			<Input id="first" name="firstName" maxlength={100} />
		</Field>
		<Field label="Last name" for="last" error={contactErrors.lastName}>
			<Input id="last" name="lastName" maxlength={100} />
		</Field>
		<Field label="Email" for="email" error={contactErrors.email}>
			<Input id="email" name="email" type="email" />
		</Field>
		<Field label="Phone" for="phone" error={contactErrors.phone}>
			<Input id="phone" name="phone" type="tel" />
		</Field>
		<div class="sm:col-span-2">
			<Field label="Job title" for="title" error={contactErrors.jobTitle}>
				<Input id="title" name="jobTitle" maxlength={200} />
			</Field>
		</div>
	</form>
	{#snippet footer()}
		<Button onclick={() => (addingContact = false)}>Cancel</Button>
		<Button type="submit" form="add-contact" variant="primary">Add contact</Button>
	{/snippet}
</Dialog>

<Dialog
	bind:open={siteOpen}
	title={editingSite ? `Edit ${editingSite.name}` : 'Add site'}
	description="Opening hours and the timezone will drive SLA business hours."
>
	{#if siteOpen}
		<form
			id="site-form"
			method="POST"
			action="?/site"
			use:enhance={saved(editingSite ? 'Site saved.' : 'Site added.', () => (siteOpen = false))}
		>
			<SiteForm site={editingSite as never} errors={siteErrors} prefix={editingSite?.id ?? 'new'} />
		</form>
	{/if}
	{#snippet footer()}
		<Button onclick={() => (siteOpen = false)}>Cancel</Button>
		<Button type="submit" form="site-form" variant="primary">Save site</Button>
	{/snippet}
</Dialog>
