<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Field, Input, Select } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { STATUSES } from './form.ts';

	type Option = { value: string; label: string };
	type Asset = components['schemas']['Asset'];
	let {
		asset,
		types,
		sites,
		contacts,
		errors = {},
		submitLabel,
		action = '?/save'
	}: {
		asset?: Asset;
		types: components['schemas']['AssetType'][];
		sites: Option[];
		contacts: Option[];
		errors?: Record<string, string>;
		submitLabel: string;
		/** The form's action (keep any query the page needs). */
		action?: string;
	} = $props();

	// svelte-ignore state_referenced_locally
	let typeId = $state(
		asset?.typeId ?? types.find((t) => t.key === 'workstation')?.id ?? types[0]?.id ?? ''
	);
	let type = $derived(types.find((t) => t.id === typeId));
	const attr = (key: string) =>
		asset && asset.typeId === typeId ? asset.attributes[key] : undefined;
	const text = (v: unknown) =>
		typeof v === 'string' || typeof v === 'number' || typeof v === 'boolean' ? String(v) : '';
</script>

<form method="POST" {action} use:enhance class="flex flex-col gap-4">
	{#if asset}<input type="hidden" name="version" value={asset.version} />{/if}
	<div class="grid gap-4 sm:grid-cols-2">
		<Field label="Name" for="asset-name" error={errors.name}>
			<Input id="asset-name" name="name" required maxlength={200} value={asset?.name ?? ''} />
		</Field>
		<Field label="Type" for="asset-type" error={errors.typeId}>
			<Select
				id="asset-type"
				name="typeId"
				bind:value={typeId}
				options={types
					.filter((t) => !t.archived || t.id === asset?.typeId)
					.map((t) => ({ value: t.id, label: t.name }))}
			/>
		</Field>
		<Field label="Status" for="asset-status" error={errors.status}>
			<Select
				id="asset-status"
				name="status"
				options={STATUSES}
				value={asset?.status ?? 'active'}
			/>
		</Field>
		<Field label="Serial number" for="asset-serial" error={errors.serialNumber}>
			<Input
				id="asset-serial"
				name="serialNumber"
				maxlength={200}
				value={asset?.serialNumber ?? ''}
			/>
		</Field>
		<Field label="Hostname" for="asset-hostname" error={errors.hostname}>
			<Input id="asset-hostname" name="hostname" maxlength={255} value={asset?.hostname ?? ''} />
		</Field>
		<Field label="Manufacturer" for="asset-manufacturer" error={errors.manufacturer}>
			<Input
				id="asset-manufacturer"
				name="manufacturer"
				maxlength={200}
				value={asset?.manufacturer ?? ''}
			/>
		</Field>
		<Field label="Model" for="asset-model" error={errors.model}>
			<Input id="asset-model" name="model" maxlength={200} value={asset?.model ?? ''} />
		</Field>
		<Field label="Site" for="asset-site" error={errors.siteId}>
			<Select
				id="asset-site"
				name="siteId"
				options={[{ value: '', label: 'None' }, ...sites]}
				value={asset?.siteId ?? ''}
			/>
		</Field>
		<Field label="Used by" for="asset-contact" error={errors.contactId}>
			<Select
				id="asset-contact"
				name="contactId"
				options={[{ value: '', label: 'Nobody' }, ...contacts]}
				value={asset?.contactId ?? ''}
			/>
		</Field>
		<Field label="Purchased on" for="asset-purchased" error={errors.purchasedOn}>
			<Input id="asset-purchased" name="purchasedOn" type="date" value={asset?.purchasedOn ?? ''} />
		</Field>
		<Field label="Warranty ends on" for="asset-warranty" error={errors.warrantyEndsOn}>
			<Input
				id="asset-warranty"
				name="warrantyEndsOn"
				type="date"
				value={asset?.warrantyEndsOn ?? ''}
			/>
		</Field>
	</div>

	{#if type && type.fields.length > 0}
		<fieldset class="grid gap-4 sm:grid-cols-2">
			<legend class="mb-2 text-sm font-medium text-fg">{type.name} details</legend>
			{#each type.fields as field (field.key)}
				{@const id = `attr-${field.key}`}
				{@const name = `attr.${field.key}`}
				{@const error = errors[`attributes/${field.key}`]}
				{#if field.fieldType === 'boolean'}
					<label class="flex items-center gap-2 text-sm text-fg">
						<input type="checkbox" {name} checked={attr(field.key) === true} />
						{field.label}
					</label>
				{:else if field.fieldType === 'select'}
					<Field label={field.label} for={id} hint={field.help} {error}>
						<Select
							{id}
							{name}
							options={[{ value: '', label: '—' }, ...(field.options ?? [])]}
							value={text(attr(field.key))}
						/>
					</Field>
				{:else if field.fieldType === 'multi_select'}
					<Field label={field.label} for={id} hint={field.help} {error}>
						<select
							{id}
							{name}
							multiple
							class="w-full rounded-control border border-border-strong bg-surface px-2 py-1 text-sm text-fg"
						>
							{#each field.options ?? [] as o (o.value)}
								<option
									value={o.value}
									selected={Array.isArray(attr(field.key)) &&
										(attr(field.key) as string[]).includes(o.value)}
								>
									{o.label}
								</option>
							{/each}
						</select>
					</Field>
				{:else}
					<Field label={field.label} for={id} hint={field.help} {error}>
						<Input
							{id}
							{name}
							required={field.required}
							type={field.fieldType === 'number'
								? 'number'
								: field.fieldType === 'date'
									? 'date'
									: field.fieldType === 'email'
										? 'email'
										: field.fieldType === 'url'
											? 'url'
											: 'text'}
							step={field.fieldType === 'number' ? 'any' : undefined}
							value={text(attr(field.key))}
						/>
					</Field>
				{/if}
			{/each}
		</fieldset>
	{/if}

	<Field label="Notes" for="asset-notes" error={errors.notes}>
		<textarea
			id="asset-notes"
			name="notes"
			rows="3"
			maxlength="10000"
			class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
			>{asset?.notes ?? ''}</textarea
		>
	</Field>
	<div><Button type="submit" variant="primary">{submitLabel}</Button></div>
</form>
