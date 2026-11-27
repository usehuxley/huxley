<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { LOCATION_KINDS, label } from '#lib/inventory/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Location = (typeof data.locations)[number];
	let open = $state(false);
	let editing: Location | null = $state(null);
	let kind = $state<string>('warehouse');
	let orgId = $state('');
	let warehouses = $derived(
		data.locations
			.filter(
				(l) => l.kind === 'warehouse' && l.id !== editing?.id && (!orgId || l.orgId === orgId)
			)
			.map((l) => ({ value: l.id, label: l.name }))
	);

	function edit(l: Location | null) {
		editing = l;
		kind = l?.kind ?? 'warehouse';
		orgId = l?.orgId ?? data.providers[0]?.value ?? '';
		open = true;
	}

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Inventory · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Locations"
		description="Where stock is kept: warehouses and the bins in them, technicians' vehicles, and stock held at client sites."
	>
		{#snippet actions()}
			<Button onclick={() => edit(null)}><Plus size={16} /> New location</Button>
		{/snippet}
		{#if data.locations.length === 0}
			<p class="text-sm text-fg-muted">No locations yet. Add a warehouse to receive stock into.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Locations">
				{#each data.locations as l (l.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-semibold text-fg">
								{l.parentName ? `${l.parentName} › ` : ''}{l.name}
								<Badge>{label(LOCATION_KINDS, l.kind).replace(' (in a warehouse)', '')}</Badge>
								{#if !l.active}<Badge>Off</Badge>{/if}
							</p>
							{#if l.assignedUserName || l.address}
								<p class="truncate text-xs text-fg-muted">
									{[l.assignedUserName, l.address].filter(Boolean).join(' · ')}
								</p>
							{/if}
						</div>
						<div class="flex shrink-0 gap-1">
							<Button size="sm" onclick={() => edit(l)}>Edit</Button>
							<form method="POST" action="?/delete" use:enhance={done('Removed.')}>
								<input type="hidden" name="id" value={l.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Remove {l.name}">
									Remove
								</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if data.thresholds.length > 0}
		<Card
			title="Purchase order approval"
			description="Orders over this total wait for someone with purchase approval before they can be sent. Leave it empty to approve every order on submit."
		>
			<div class="flex flex-col gap-3">
				{#each data.thresholds as t (t.orgId)}
					<form
						method="POST"
						action="?/threshold"
						class="flex flex-wrap items-end gap-2"
						use:enhance={done('Saved.')}
					>
						<input type="hidden" name="orgId" value={t.orgId} />
						<Field
							label={data.thresholds.length > 1
								? `Approve over (${t.name})`
								: 'Approve orders over'}
							for="threshold-{t.orgId}"
						>
							<Input
								id="threshold-{t.orgId}"
								name="approvalAbove"
								inputmode="decimal"
								value={t.approvalAbove}
								class="w-40"
							/>
						</Field>
						<Button type="submit">Save</Button>
					</form>
				{/each}
			</div>
		</Card>
	{/if}
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New location'}>
	<form
		id="location-form"
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done('Saved.', () => (open = false))}
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
		{:else if data.providers.length > 1}
			<Field label="Organization" for="location-org">
				<Select id="location-org" name="orgId" bind:value={orgId} options={data.providers} />
			</Field>
		{:else}
			<input type="hidden" name="orgId" value={orgId} />
		{/if}
		<Field label="Name" for="location-name">
			<Input id="location-name" name="name" required maxlength={100} value={editing?.name ?? ''} />
		</Field>
		<Field label="Kind" for="location-kind">
			<Select
				id="location-kind"
				name="kind"
				bind:value={kind}
				options={LOCATION_KINDS.map((k) => ({ ...k }))}
			/>
		</Field>
		{#if kind === 'bin'}
			<Field label="In warehouse" for="location-parent">
				<Select
					id="location-parent"
					name="parentId"
					required
					value={editing?.parentId ?? ''}
					options={warehouses}
				/>
			</Field>
		{:else if kind === 'vehicle'}
			<Field label="Technician" for="location-user">
				<Select
					id="location-user"
					name="assignedUserId"
					value={editing?.assignedUserId ?? ''}
					options={[{ value: '', label: 'Nobody yet' }, ...data.people]}
				/>
			</Field>
		{:else}
			<Field label="Address" for="location-address">
				<Input
					id="location-address"
					name="address"
					maxlength={500}
					value={editing?.address ?? ''}
				/>
			</Field>
		{/if}
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="active" checked={editing?.active ?? true} /> In use
		</label>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="location-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
