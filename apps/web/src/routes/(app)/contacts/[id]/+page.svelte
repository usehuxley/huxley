<script lang="ts">
	import PortalAccessCard from '#lib/portal/PortalAccessCard.svelte';
	import ActionsCard from '#lib/forms/ActionsCard.svelte';
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import FieldInput from '#lib/fields/FieldInput.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let contact = $derived(data.contact);
	let canManage = $derived(data.user.grantedPermissions.includes('crm:manage'));
	let primary = $derived(contact.companies.find((c) => c.isPrimary) ?? null);
	let addable = $derived(
		data.companies
			.filter((c) => !contact.companies.some((mine) => mine.orgId === c.orgId))
			.map((c) => ({ value: c.orgId, label: c.name }))
	);
	let site = $state('');
	let adding = $state('');
	$effect(() => {
		site = data.contact.siteId ?? '';
		adding = '';
	});

	let detailErrors: Record<string, string> = $derived(
		(form?.details && 'errors' in form.details ? form.details.errors : null) ?? {}
	);
	let fieldErrors: Record<string, string> = $derived(
		(form?.fields && 'errors' in form.fields ? form.fields.errors : null) ?? {}
	);

	const saved =
		(done: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') toast.push({ title: done, tone: 'success' });
			else if (result.type === 'failure') {
				const data = result.data as Record<string, { message?: string }> | undefined;
				const message = Object.values(data ?? {})[0]?.message;
				if (message) toast.push({ title: message, tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>{contact.name} · Contacts · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div>
		{#if primary}
			<a
				href={resolve('/(app)/clients/[id]', { id: primary.orgId })}
				class="text-sm text-fg-muted hover:text-fg"
			>
				← {primary.orgName}
			</a>
		{/if}
		<div class="mt-2 flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-fg">{contact.name}</h1>
			{#if contact.archivedAt}<Badge>Archived</Badge>{/if}
		</div>
		{#if primary?.jobTitle}<p class="mt-1 text-fg-muted">{primary.jobTitle}</p>{/if}
	</div>

	<ActionsCard actions={data.actions} orgId={contact.orgId} contactId={contact.id} />

	{#if data.portal}
		<PortalAccessCard
			account={data.portal.account}
			hasEmail={!!data.contact.email}
			canManage={data.portal.canManage}
		/>
	{/if}

	<Card title="Details">
		<form
			method="POST"
			action="?/details"
			class="grid gap-4 sm:grid-cols-2"
			use:enhance={saved('Contact saved.')}
		>
			<input type="hidden" name="version" value={contact.version} />
			<Field label="First name" for="first" error={detailErrors.firstName}>
				<Input id="first" name="firstName" value={contact.firstName} disabled={!canManage} />
			</Field>
			<Field label="Last name" for="last" error={detailErrors.lastName}>
				<Input id="last" name="lastName" value={contact.lastName} disabled={!canManage} />
			</Field>
			<Field label="Email" for="email" error={detailErrors.email}>
				<Input
					id="email"
					name="email"
					type="email"
					value={contact.email ?? ''}
					disabled={!canManage}
				/>
			</Field>
			<Field label="Job title" for="title" error={detailErrors.jobTitle}>
				<Input id="title" name="jobTitle" value={primary?.jobTitle ?? ''} disabled={!canManage} />
			</Field>
			<Field label="Phone" for="phone" error={detailErrors.phone}>
				<Input
					id="phone"
					name="phone"
					type="tel"
					value={contact.phone ?? ''}
					disabled={!canManage}
				/>
			</Field>
			<Field label="Mobile" for="mobile" error={detailErrors.mobile}>
				<Input
					id="mobile"
					name="mobile"
					type="tel"
					value={contact.mobile ?? ''}
					disabled={!canManage}
				/>
			</Field>
			<Field label="Site" for="site" error={detailErrors.siteId}>
				<Select
					id="site"
					name="siteId"
					bind:value={site}
					placeholder="No site"
					options={[
						{ value: '', label: 'No site' },
						...data.sites.map((s) => ({ value: s.id, label: s.name }))
					]}
					disabled={!canManage}
				/>
			</Field>
			<Field
				label="Entra ID object id"
				for="entra"
				hint="Matches them to their Microsoft 365 account"
				error={detailErrors.externalIds}
			>
				<Input
					id="entra"
					name="entra"
					value={(contact.externalIds as Record<string, string>).entra ?? ''}
					disabled={!canManage}
				/>
			</Field>
			{#if canManage}
				<div class="flex gap-2 sm:col-span-2">
					<Button type="submit" variant="primary">Save contact</Button>
				</div>
			{/if}
		</form>
	</Card>

	<Card
		title="Companies"
		description="Where they work. The primary company is the one they belong to: only people who can see it can see them."
	>
		<form
			method="POST"
			action="?/companies"
			class="flex flex-col gap-3"
			use:enhance={saved('Companies changed.')}
		>
			<input type="hidden" name="version" value={contact.version} />
			<ul class="flex flex-col divide-y divide-border" aria-label="Companies">
				{#each contact.companies as company (company.orgId)}
					<li class="grid items-center gap-3 py-2 sm:grid-cols-[1fr_14rem_auto]">
						<input type="hidden" name="orgId" value={company.orgId} />
						<a
							class="font-medium text-fg hover:text-accent"
							href={resolve('/(app)/clients/[id]', { id: company.orgId })}
						>
							{company.orgName}
						</a>
						<label class="sr-only" for="title-{company.orgId}">
							Job title at {company.orgName}
						</label>
						<Input
							id="title-{company.orgId}"
							name="title_{company.orgId}"
							value={company.jobTitle ?? ''}
							placeholder="Job title"
							disabled={!canManage}
						/>
						<label class="flex items-center gap-2 text-sm text-fg">
							<input
								type="radio"
								name="primary"
								value={company.orgId}
								checked={company.isPrimary}
								disabled={!canManage}
							/>
							Primary
						</label>
					</li>
				{/each}
			</ul>
			{#if canManage}
				<div class="flex flex-wrap items-end gap-2">
					{#if addable.length > 0}
						<Field label="Add a company" for="add">
							<Select
								id="add"
								name="add"
								bind:value={adding}
								placeholder="Choose a company"
								options={addable}
							/>
						</Field>
					{/if}
					<Button type="submit">Save companies</Button>
				</div>
			{/if}
		</form>
	</Card>

	{#if data.fields.length > 0}
		<Card title="Custom fields">
			<form
				method="POST"
				action="?/fields"
				class="flex flex-col gap-4"
				use:enhance={saved('Fields saved.')}
			>
				<div class="grid gap-4 sm:grid-cols-2">
					{#each data.fields as field (field.id)}
						<FieldInput {field} disabled={!canManage} error={fieldErrors[field.key]} />
					{/each}
				</div>
				{#if canManage}<div><Button type="submit" variant="primary">Save fields</Button></div>{/if}
			</form>
		</Card>
	{/if}

	{#if canManage && !contact.archivedAt}
		<form method="POST" action="?/archive">
			<input type="hidden" name="orgId" value={contact.orgId} />
			<Button type="submit" variant="ghost">Archive contact</Button>
		</form>
	{/if}
</div>
