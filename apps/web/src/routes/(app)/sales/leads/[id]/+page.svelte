<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowRightLeft, Ban } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import OpportunityFields from '#lib/sales/OpportunityFields.svelte';
	import { LEAD_STATUSES, label, leadTone } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let lead = $derived(data.lead);
	let editable = $derived(data.canWrite && lead.status !== 'converted');
	let converting = $state(false);
	let disqualifying = $state(false);
	let saveErrors = $derived(
		((form && 'save' in form ? form.save?.errors : undefined) ?? {}) as Record<string, string>
	);
	let convertErrors = $derived(
		(form && 'convert' in form ? form.convert?.errors : undefined) ?? {}
	);
	$effect(() => {
		if (form && 'convert' in form) converting = true;
	});

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = Object.values(result.data ?? {})[0] as { message?: string } | undefined;
				if (failed?.message) toast.push({ title: failed.message, tone: 'danger' });
			}
		};
	const textarea =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg';
</script>

<svelte:head>
	<title>{lead.companyName} · Leads · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/sales/leads')} class="text-sm text-fg-muted hover:text-fg">← Leads</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<h1 class="text-2xl font-extrabold tracking-tight text-fg">{lead.companyName}</h1>
				<div class="mt-2 flex flex-wrap items-center gap-2">
					<Badge tone={leadTone(lead.status)}>{label(LEAD_STATUSES, lead.status)}</Badge>
					{#if lead.source}<Badge>{lead.source}</Badge>{/if}
				</div>
			</div>
			{#if editable}
				<div class="flex gap-2">
					{#if lead.status !== 'disqualified'}
						<Button variant="primary" onclick={() => (converting = true)}>
							<ArrowRightLeft size={16} /> Convert
						</Button>
						<Button onclick={() => (disqualifying = true)}><Ban size={16} /> Disqualify</Button>
					{/if}
				</div>
			{/if}
		</div>
	</div>

	{#if lead.status === 'converted' && lead.convertedOpportunityId}
		<div
			role="status"
			class="rounded-card border border-success/40 bg-success-soft p-4 text-sm text-fg"
		>
			Converted:
			<a
				href={resolve('/(app)/sales/opportunities/[id]', { id: lead.convertedOpportunityId })}
				class="font-semibold text-accent hover:underline"
			>
				the opportunity
			</a>
			{#if lead.convertedOrgId}
				· <a
					href={resolve('/(app)/clients/[id]', { id: lead.convertedOrgId })}
					class="font-semibold text-accent hover:underline"
				>
					the client
				</a>
			{/if}
		</div>
	{:else if lead.status === 'disqualified'}
		<div
			role="status"
			class="rounded-card border border-border bg-surface-sunken p-4 text-sm text-fg"
		>
			Disqualified: {lead.disqualifiedReason}. Set its status back to Working to pursue it.
		</div>
	{/if}

	<Card title="Details">
		<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance={done('Saved.')}>
			<input type="hidden" name="version" value={lead.version} />
			<fieldset disabled={!editable} class="flex flex-col gap-4">
				<Field label="Company" for="companyName" error={saveErrors.companyName}>
					<Input id="companyName" name="companyName" required value={lead.companyName} />
				</Field>
				<div class="grid gap-4 sm:grid-cols-2">
					<Field label="First name" for="firstName">
						<Input id="firstName" name="firstName" value={lead.firstName ?? ''} />
					</Field>
					<Field label="Last name" for="lastName">
						<Input id="lastName" name="lastName" value={lead.lastName ?? ''} />
					</Field>
					<Field label="Email" for="email" error={saveErrors.email}>
						<Input id="email" name="email" value={lead.email ?? ''} />
					</Field>
					<Field label="Phone" for="phone">
						<Input id="phone" name="phone" value={lead.phone ?? ''} />
					</Field>
					<Field label="Source" for="source">
						<Input id="source" name="source" value={lead.source ?? ''} />
					</Field>
					<Field label="Owner" for="ownerId">
						<Select
							id="ownerId"
							name="ownerId"
							value={lead.ownerId ?? ''}
							options={[{ value: '', label: 'Nobody' }, ...data.people]}
						/>
					</Field>
					{#if lead.status === 'new' || lead.status === 'working' || lead.status === 'disqualified'}
						<Field label="Status" for="status">
							<Select
								id="status"
								name="status"
								value={lead.status === 'disqualified' ? '' : lead.status}
								options={[
									...(lead.status === 'disqualified' ? [{ value: '', label: 'Disqualified' }] : []),
									{ value: 'new', label: 'New' },
									{ value: 'working', label: 'Working' }
								]}
							/>
						</Field>
					{/if}
				</div>
				<Field label="Notes" for="notes">
					<textarea id="notes" name="notes" rows="5" class={textarea}>{lead.notes}</textarea>
				</Field>
			</fieldset>
			{#if editable}
				<div class="flex justify-between">
					<Button type="submit" variant="primary">Save</Button>
					<Button type="submit" formaction="?/delete" variant="ghost">Delete</Button>
				</div>
			{/if}
		</form>
	</Card>
</div>

<Dialog
	bind:open={converting}
	title="Convert {lead.companyName}"
	description="Makes the client, a contact and an opportunity."
>
	<form id="convert-form" method="POST" action="?/convert" class="flex flex-col gap-4" use:enhance>
		<input type="hidden" name="version" value={lead.version} />
		<Field label="Client" for="clientId" hint="Leave as new to make {lead.companyName} a client">
			<Select
				id="clientId"
				name="clientId"
				options={[{ value: '', label: `New: ${lead.companyName}` }, ...data.clients]}
			/>
		</Field>
		{#if lead.firstName || lead.lastName}
			<label class="flex items-center gap-2 text-sm font-medium text-fg">
				<input type="checkbox" name="createContact" checked />
				Add {[lead.firstName, lead.lastName].filter(Boolean).join(' ')} as a contact
			</label>
		{/if}
		<OpportunityFields
			values={{
				name: `${lead.companyName} opportunity`,
				source: lead.source,
				ownerId: lead.ownerId
			}}
			people={data.people}
			errors={convertErrors}
		/>
		{#if form && 'convert' in form && form.convert?.message}
			<p class="text-sm text-danger" role="alert">{form.convert.message}</p>
		{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (converting = false)}>Cancel</Button>
		<Button type="submit" form="convert-form" variant="primary">Convert</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={disqualifying} title="Disqualify {lead.companyName}">
	<form
		id="dq-form"
		method="POST"
		action="?/disqualify"
		class="flex flex-col gap-4"
		use:enhance={done('Disqualified.', () => (disqualifying = false))}
	>
		<input type="hidden" name="version" value={lead.version} />
		<Field label="Why" for="reason">
			<Input
				id="reason"
				name="reason"
				required
				maxlength={500}
				placeholder="No budget, wrong size, student project…"
			/>
		</Field>
	</form>
	{#snippet footer()}
		<Button onclick={() => (disqualifying = false)}>Cancel</Button>
		<Button type="submit" form="dq-form" variant="primary">Disqualify</Button>
	{/snippet}
</Dialog>
