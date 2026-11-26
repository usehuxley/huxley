<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Package, Plus, Trash2 } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		Dialog,
		EmptyState,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import type { PageProps } from './$types';

	type Product = components['schemas']['CatalogProduct'];
	type Kind = components['schemas']['CatalogKind'];
	type Billing = components['schemas']['CatalogBilling'];

	let { data, form }: PageProps = $props();
	let canManage = $derived(data.user.grantedPermissions.includes('catalog:manage'));
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	let manyOrgs = $derived(new Set(data.products.map((p) => p.orgId)).size > 1);

	const KINDS: { value: Kind; label: string }[] = [
		{ value: 'product', label: 'Product' },
		{ value: 'service', label: 'Service' },
		{ value: 'bundle', label: 'Bundle' }
	];
	const BILLING: { value: Billing; label: string }[] = [
		{ value: 'one_time', label: 'One time' },
		{ value: 'monthly', label: 'Monthly' },
		{ value: 'quarterly', label: 'Quarterly' },
		{ value: 'annual', label: 'Annual' }
	];
	const billingLabel = (b: Billing) => BILLING.find((x) => x.value === b)?.label ?? b;

	function money(amount: string | null | undefined, currency: string): string {
		if (amount === null || amount === undefined || amount === '') return '—';
		const n = Number(amount);
		try {
			return new Intl.NumberFormat(undefined, {
				style: 'currency',
				currency,
				maximumFractionDigits: 4
			}).format(n);
		} catch {
			return `${amount} ${currency}`;
		}
	}

	// Filters: words, kind, category, and whether retired ones show.
	let query = $state('');
	let kindFilter = $state('');
	let category = $state('');
	let showRetired = $state(false);
	let categories = $derived(
		[...new Set(data.products.map((p) => p.category).filter((c): c is string => !!c))].sort()
	);
	let shown = $derived(
		data.products.filter((p) => {
			const q = query.trim().toLowerCase();
			if (q && !p.name.toLowerCase().includes(q) && !p.sku.toLowerCase().startsWith(q))
				return false;
			if (kindFilter && p.kind !== kindFilter) return false;
			if (category && p.category !== category) return false;
			return showRetired || p.active;
		})
	);

	// The product being added or changed.
	let open = $state(false);
	let editing = $state<Product | null>(null);
	let orgId = $state('');
	let sku = $state('');
	let name = $state('');
	let description = $state('');
	let kind = $state<Kind>('product');
	let productCategory = $state('');
	let vendor = $state('');
	let manufacturerPart = $state('');
	let unit = $state('each');
	let currency = $state('USD');
	let cost = $state('');
	let price = $state('');
	let billing = $state<Billing>('one_time');
	let taxable = $state(true);
	let active = $state(true);
	let parts: { productId: string; quantity: string }[] = $state([]);
	let errors: Record<string, string> = $derived(
		form && 'errors' in form ? (form.errors ?? {}) : {}
	);
	let partOptions = $derived([
		{ value: '', label: 'Choose a product' },
		...data.products
			.filter((p) => p.kind !== 'bundle' && p.orgId === orgId && p.id !== editing?.id)
			.map((p) => ({ value: p.id, label: `${p.name} (${p.sku})` }))
	]);

	function edit(p: Product | null) {
		editing = p;
		orgId = p?.orgId ?? data.currentOrg?.id ?? data.orgs[0]?.id ?? '';
		sku = p?.sku ?? '';
		name = p?.name ?? '';
		description = p?.description ?? '';
		kind = p?.kind ?? 'product';
		productCategory = p?.category ?? '';
		vendor = p?.vendor ?? '';
		manufacturerPart = p?.manufacturerPart ?? '';
		unit = p?.unit ?? 'each';
		currency = p?.currency ?? 'USD';
		cost = p?.cost ?? '';
		price = p?.price ?? '';
		billing = p?.billing ?? 'one_time';
		taxable = p?.taxable ?? true;
		active = p?.active ?? true;
		parts = (p?.components ?? []).map((c) => ({ productId: c.productId, quantity: c.quantity }));
		open = true;
	}

	let productJson = $derived(
		JSON.stringify({
			sku,
			name,
			description,
			kind,
			category: productCategory || null,
			vendor: vendor || null,
			manufacturerPart: manufacturerPart || null,
			unit,
			currency,
			cost: cost || null,
			price: price || null,
			billing,
			taxable,
			active,
			components: kind === 'bundle' ? parts.filter((c) => c.productId) : [],
			...(editing ? { version: editing.version } : {})
		})
	);

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const saved = (result.data as { saved?: string } | undefined)?.saved ?? 'Saved.';
				toast.push({ title: saved, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failure = result.data as { message?: string } | undefined;
				toast.push({ title: failure?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Catalog · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Catalog</h1>
			<p class="mt-1 text-sm text-fg-muted">
				The products and services you sell, and bundles of them, with their costs and prices.
			</p>
		</div>
		{#if canManage}
			<Button onclick={() => edit(null)}><Plus size={16} /> New product</Button>
		{/if}
	</header>

	<Card>
		<div class="mb-4 flex flex-wrap items-end gap-3">
			<div class="w-64">
				<Field label="Search the catalog" for="cat-search">
					<Input id="cat-search" type="search" bind:value={query} placeholder="Name or SKU" />
				</Field>
			</div>
			<div class="w-40">
				<Field label="Kind" for="cat-kind">
					<Select
						id="cat-kind"
						bind:value={kindFilter}
						options={[{ value: '', label: 'Any' }, ...KINDS]}
					/>
				</Field>
			</div>
			{#if categories.length > 0}
				<div class="w-48">
					<Field label="Category" for="cat-category">
						<Select
							id="cat-category"
							bind:value={category}
							options={[
								{ value: '', label: 'Any' },
								...categories.map((c) => ({ value: c, label: c }))
							]}
						/>
					</Field>
				</div>
			{/if}
			<label class="flex items-center gap-2 pb-2 text-sm text-fg">
				<input type="checkbox" bind:checked={showRetired} /> Show retired
			</label>
		</div>

		{#if shown.length === 0}
			<EmptyState
				title={data.products.length === 0 ? 'Nothing in the catalog yet' : 'Nothing matches'}
				description={data.products.length === 0
					? 'Add the products and services you sell.'
					: 'Try other words or filters.'}
			>
				{#snippet icon()}<Package size={20} />{/snippet}
			</EmptyState>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-sm" aria-label="Products">
					<thead class="text-left text-fg-muted">
						<tr>
							<th class="py-2 pr-3 font-medium">Name</th>
							<th class="py-2 pr-3 font-medium">SKU</th>
							<th class="py-2 pr-3 font-medium">Kind</th>
							<th class="py-2 pr-3 text-right font-medium">Cost</th>
							<th class="py-2 pr-3 text-right font-medium">Price</th>
							<th class="py-2 font-medium">Billing</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each shown as p (p.id)}
							<tr>
								<td class="py-2 pr-3">
									{#if canManage}
										<button
											type="button"
											class="text-left font-medium text-fg hover:underline"
											onclick={() => edit(p)}
										>
											{p.name}
										</button>
									{:else}
										<span class="font-medium text-fg">{p.name}</span>
									{/if}
									{#if !p.active}<Badge tone="neutral">Retired</Badge>{/if}
									<p class="text-xs text-fg-muted">
										{[p.vendor, p.category, manyOrgs ? orgName.get(p.orgId) : null]
											.filter(Boolean)
											.join(' · ')}
									</p>
									{#if p.kind === 'bundle' && p.components.length > 0}
										<p class="text-xs text-fg-muted">
											{p.components.map((c) => `${c.quantity} × ${c.name}`).join(', ')}
										</p>
									{/if}
								</td>
								<td class="py-2 pr-3 font-mono text-xs">{p.sku}</td>
								<td class="py-2 pr-3 capitalize">{p.kind}</td>
								<td class="py-2 pr-3 text-right tabular-nums">{money(p.cost, p.currency)}</td>
								<td class="py-2 pr-3 text-right tabular-nums">
									{#if p.price}
										{money(p.price, p.currency)}
									{:else if p.componentsPrice}
										<span title="Its components’ prices added up">
											{money(p.componentsPrice, p.currency)}
										</span>
									{:else}—{/if}
									<span class="text-xs text-fg-muted">/ {p.unit}</span>
								</td>
								<td class="py-2">{billingLabel(p.billing)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New product'}>
	<form
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done(() => (open = false))}
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<input type="hidden" name="orgId" value={orgId} />
		<input type="hidden" name="product" value={productJson} />
		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Name" for="p-name" error={errors.name}>
				<Input id="p-name" bind:value={name} required maxlength={200} />
			</Field>
			<Field label="SKU" for="p-sku" error={errors.sku}>
				<Input id="p-sku" bind:value={sku} required maxlength={64} />
			</Field>
			<Field label="Kind" for="p-kind" error={errors.kind}>
				<Select id="p-kind" bind:value={kind} options={KINDS} />
			</Field>
			{#if !editing && data.orgs.length > 1}
				<Field label="Organization" for="p-org">
					<Select
						id="p-org"
						bind:value={orgId}
						options={data.orgs.map((o) => ({ value: o.id, label: o.name }))}
					/>
				</Field>
			{/if}
			<Field label="Category" for="p-category" error={errors.category}>
				<Input id="p-category" bind:value={productCategory} maxlength={100} />
			</Field>
			<Field label="Vendor" for="p-vendor" error={errors.vendor}>
				<Input id="p-vendor" bind:value={vendor} maxlength={200} />
			</Field>
			<Field label="Manufacturer part number" for="p-mpn" error={errors.manufacturerPart}>
				<Input id="p-mpn" bind:value={manufacturerPart} maxlength={100} />
			</Field>
			<Field label="Unit" for="p-unit" hint="each, hour, month, user, device…" error={errors.unit}>
				<Input id="p-unit" bind:value={unit} maxlength={32} />
			</Field>
			<Field label="Cost" for="p-cost" error={errors.cost}>
				<Input id="p-cost" bind:value={cost} inputmode="decimal" placeholder="0.00" />
			</Field>
			<Field
				label="Price"
				for="p-price"
				hint={kind === 'bundle' ? 'Leave empty to add up its components' : undefined}
				error={errors.price}
			>
				<Input id="p-price" bind:value={price} inputmode="decimal" placeholder="0.00" />
			</Field>
			<Field label="Currency" for="p-currency" error={errors.currency}>
				<Input id="p-currency" bind:value={currency} maxlength={3} />
			</Field>
			<Field label="Billing" for="p-billing" error={errors.billing}>
				<Select id="p-billing" bind:value={billing} options={BILLING} />
			</Field>
		</div>
		<Field label="Description" for="p-description" error={errors.description}>
			<textarea
				id="p-description"
				bind:value={description}
				rows="3"
				maxlength="4000"
				class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
			></textarea>
		</Field>

		{#if kind === 'bundle'}
			<fieldset class="flex flex-col gap-2">
				<legend class="mb-1 text-sm font-medium text-fg">In the bundle</legend>
				{#if errors.components}<p class="text-sm text-danger">{errors.components}</p>{/if}
				{#each parts as part, i (i)}
					<div class="flex items-end gap-2" role="group" aria-label="Component {i + 1}">
						<div class="min-w-0 flex-1">
							<Field label="Product" for="part-{i}" error={errors[`components/${i}/productId`]}>
								<Select id="part-{i}" bind:value={part.productId} options={partOptions} />
							</Field>
						</div>
						<div class="w-24">
							<Field label="Quantity" for="qty-{i}" error={errors[`components/${i}/quantity`]}>
								<Input id="qty-{i}" bind:value={part.quantity} inputmode="decimal" />
							</Field>
						</div>
						<button
							type="button"
							class="pb-2 text-fg-subtle hover:text-danger"
							aria-label="Remove component {i + 1}"
							onclick={() => (parts = parts.filter((_, j) => j !== i))}
						>
							<Trash2 size={16} />
						</button>
					</div>
				{/each}
				<div>
					<Button
						size="sm"
						variant="secondary"
						onclick={() => (parts = [...parts, { productId: '', quantity: '1' }])}
					>
						<Plus size={14} /> Add a product
					</Button>
				</div>
			</fieldset>
		{/if}

		<div class="flex flex-wrap gap-4">
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" bind:checked={taxable} /> Taxable
			</label>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" bind:checked={active} /> Offered (untick to retire it)
			</label>
		</div>
		<div class="flex justify-between gap-2">
			<div>
				{#if editing}
					<Button type="submit" variant="danger" formaction="?/delete">Delete</Button>
				{/if}
			</div>
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</div>
	</form>
</Dialog>
