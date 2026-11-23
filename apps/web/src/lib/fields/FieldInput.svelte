<script lang="ts">
	import type { components } from '@usehuxley/api-client';
	import { Field, Input, Select } from '@usehuxley/ui';

	type FieldValue = components['schemas']['FieldValue'];

	let {
		field,
		disabled = false,
		error
	}: { field: FieldValue; disabled?: boolean; error?: string } = $props();

	const id = $derived(`f-${field.key}`);
	const name = $derived(`f:${field.key}`);
	const value = $derived(field.value as unknown);
	const asText = (v: unknown) => (typeof v === 'string' || typeof v === 'number' ? String(v) : '');
</script>

{#if field.fieldType === 'boolean'}
	<label class="flex items-center gap-2 text-sm text-fg">
		<input type="checkbox" {id} {name} checked={value === true} {disabled} />
		{field.label}
	</label>
{:else if field.fieldType === 'multi_select'}
	<fieldset class="flex flex-col gap-1.5">
		<legend class="text-sm font-medium text-fg">{field.label}</legend>
		{#each field.options as option (option.value)}
			<label class="flex items-center gap-2 text-sm text-fg">
				<input
					type="checkbox"
					{name}
					value={option.value}
					checked={Array.isArray(value) && value.includes(option.value)}
					{disabled}
				/>
				{option.label}
			</label>
		{/each}
		{#if error}<p class="text-sm text-danger">{error}</p>{/if}
	</fieldset>
{:else}
	<Field
		label={field.label}
		for={id}
		hint={field.help || undefined}
		{error}
		required={field.required}
	>
		{#if field.fieldType === 'long_text'}
			<textarea
				{id}
				{name}
				{disabled}
				rows="3"
				class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				>{asText(value)}</textarea
			>
		{:else if field.fieldType === 'select'}
			<Select
				{id}
				{name}
				{disabled}
				value={asText(value)}
				options={[
					// An empty choice clears an optional field.
					...(field.required ? [] : [{ value: '', label: '—' }]),
					...field.options.map((o) => ({ value: o.value, label: o.label }))
				]}
			/>
		{:else}
			<Input
				{id}
				{name}
				{disabled}
				value={asText(value)}
				type={field.fieldType === 'number'
					? 'number'
					: field.fieldType === 'date'
						? 'date'
						: field.fieldType === 'url'
							? 'url'
							: field.fieldType === 'email'
								? 'email'
								: 'text'}
				step={field.fieldType === 'number' ? 'any' : undefined}
			/>
		{/if}
	</Field>
{/if}
