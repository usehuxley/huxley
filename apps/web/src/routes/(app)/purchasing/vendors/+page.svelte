<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Dialog, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	type Vendor = (typeof data.vendors)[number];
	let open = $state(false);
	let editing: Vendor | null = $state(null);

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
	<title>Vendors · Purchasing · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div class="flex flex-wrap items-end gap-4">
		<div>
			<a href={resolve('/(app)/purchasing')} class="text-sm text-fg-muted hover:text-fg">
				← Purchasing
			</a>
			<h1 class="mt-1 text-[26px] leading-tight font-extrabold tracking-tight text-fg">Vendors</h1>
			<p class="mt-1 text-sm text-fg-subtle">
				Who you buy from. A catalog product whose vendor matches a name here is reordered from it.
			</p>
		</div>
		{#if data.canWrite}
			<div class="ml-auto">
				<Button variant="primary" onclick={() => ((editing = null), (open = true))}>
					<Plus size={16} /> New vendor
				</Button>
			</div>
		{/if}
	</div>

	{#if data.vendors.length === 0}
		<EmptyState
			title="No vendors yet"
			description="Add the distributors and suppliers you order from."
		/>
	{:else}
		<ul
			class="flex flex-col divide-y divide-border rounded-card border border-border bg-surface"
			aria-label="Vendors"
		>
			{#each data.vendors as v (v.id)}
				<li class="flex items-center justify-between gap-3 px-4 py-3">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-semibold text-fg">
							{v.name}
							{#if !v.active}<Badge>Off</Badge>{/if}
							{#if v.openOrders > 0}<Badge tone="info">{v.openOrders} open</Badge>{/if}
						</p>
						<p class="truncate text-xs text-fg-muted">
							{[
								v.email,
								v.phone,
								v.accountNumber ? `Account ${v.accountNumber}` : null,
								v.paymentTerms
							]
								.filter(Boolean)
								.join(' · ')}
						</p>
					</div>
					{#if data.canWrite}
						<div class="flex shrink-0 gap-1">
							<Button size="sm" onclick={() => ((editing = v), (open = true))}>Edit</Button>
							<form method="POST" action="?/delete" use:enhance={done('Removed.')}>
								<input type="hidden" name="id" value={v.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Remove {v.name}">
									Remove
								</Button>
							</form>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New vendor'}>
	<form
		id="vendor-form"
		method="POST"
		action="?/save"
		class="grid gap-4 sm:grid-cols-2"
		use:enhance={done('Saved.', () => (open = false))}
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
		{:else if data.providers.length > 1}
			<div class="sm:col-span-2">
				<Field label="Organization" for="vendor-org">
					<Select id="vendor-org" name="orgId" options={data.providers} />
				</Field>
			</div>
		{:else}
			<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
		{/if}
		<div class="sm:col-span-2">
			<Field label="Name" for="vendor-name">
				<Input id="vendor-name" name="name" required maxlength={200} value={editing?.name ?? ''} />
			</Field>
		</div>
		<Field label="Orders email" for="vendor-email">
			<Input id="vendor-email" name="email" type="email" value={editing?.email ?? ''} />
		</Field>
		<Field label="Phone" for="vendor-phone">
			<Input id="vendor-phone" name="phone" value={editing?.phone ?? ''} />
		</Field>
		<Field label="Your account number" for="vendor-account">
			<Input id="vendor-account" name="accountNumber" value={editing?.accountNumber ?? ''} />
		</Field>
		<Field label="Payment terms" for="vendor-terms">
			<Input
				id="vendor-terms"
				name="paymentTerms"
				placeholder="e.g. Net 30"
				value={editing?.paymentTerms ?? ''}
			/>
		</Field>
		<Field label="Currency" for="vendor-currency">
			<Input
				id="vendor-currency"
				name="currency"
				maxlength={3}
				value={editing?.currency ?? 'USD'}
			/>
		</Field>
		<Field label="Website" for="vendor-website">
			<Input id="vendor-website" name="website" value={editing?.website ?? ''} />
		</Field>
		<div class="sm:col-span-2">
			<Field label="Notes" for="vendor-notes">
				<Input id="vendor-notes" name="notes" value={editing?.notes ?? ''} />
			</Field>
		</div>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="active" checked={editing?.active ?? true} /> In use
		</label>
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="vendor-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
