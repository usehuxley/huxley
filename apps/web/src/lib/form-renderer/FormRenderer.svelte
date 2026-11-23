<!--
	Renders a form's fields from its JSON Schema and UI hints, with the
	answers in `values` and per-field `errors` (by field name).
-->
<script lang="ts">
	import { fieldsOf, type Field } from './fields.ts';

	let {
		schema,
		ui = {},
		values = $bindable({}),
		errors = {},
		idPrefix = 'field'
	}: {
		schema: unknown;
		ui?: unknown;
		values?: Record<string, unknown>;
		errors?: Record<string, string>;
		idPrefix?: string;
	} = $props();

	let fields = $derived(fieldsOf(schema, ui));
	const id = (f: Field) => `${idPrefix}-${f.name}`;
	const inputClass = 'rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg';

	function toggle(field: Field, option: string, on: boolean) {
		const current = Array.isArray(values[field.name]) ? (values[field.name] as string[]) : [];
		values[field.name] = on ? [...current, option] : current.filter((o) => o !== option);
	}
	const str = (v: unknown) => (typeof v === 'string' || typeof v === 'number' ? String(v) : '');
</script>

<div class="flex flex-col gap-4">
	{#each fields as field (field.name)}
		<div class="flex flex-col gap-1">
			{#if field.kind === 'boolean'}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input
						id={id(field)}
						type="checkbox"
						checked={values[field.name] === true}
						onchange={(e) => (values[field.name] = e.currentTarget.checked)}
					/>
					{field.label}
				</label>
			{:else if field.kind === 'multiselect'}
				<fieldset class="flex flex-col gap-1">
					<legend class="text-sm font-medium text-fg">
						{field.label}{field.required ? ' *' : ''}
					</legend>
					{#each field.options as option (option)}
						<label class="flex items-center gap-2 text-sm text-fg">
							<input
								type="checkbox"
								checked={Array.isArray(values[field.name]) &&
									(values[field.name] as string[]).includes(option)}
								onchange={(e) => toggle(field, option, e.currentTarget.checked)}
							/>
							{option}
						</label>
					{/each}
				</fieldset>
			{:else}
				<label for={id(field)} class="text-sm font-medium text-fg">
					{field.label}{field.required ? ' *' : ''}
				</label>
				{#if field.kind === 'select'}
					<select
						id={id(field)}
						class={inputClass}
						value={str(values[field.name])}
						required={field.required}
						onchange={(e) => (values[field.name] = e.currentTarget.value)}
					>
						<option value="">Choose…</option>
						{#each field.options as option (option)}<option value={option}>{option}</option>{/each}
					</select>
				{:else if field.kind === 'textarea'}
					<textarea
						id={id(field)}
						class={inputClass}
						rows="4"
						placeholder={field.placeholder}
						required={field.required}
						value={str(values[field.name])}
						oninput={(e) => (values[field.name] = e.currentTarget.value)}></textarea>
				{:else}
					<input
						id={id(field)}
						class={inputClass}
						type={field.kind === 'email'
							? 'email'
							: field.kind === 'date'
								? 'date'
								: field.kind === 'datetime'
									? 'datetime-local'
									: field.kind === 'number' || field.kind === 'integer'
										? 'number'
										: 'text'}
						step={field.kind === 'integer' ? '1' : field.kind === 'number' ? 'any' : undefined}
						placeholder={field.placeholder}
						required={field.required}
						value={str(values[field.name])}
						oninput={(e) => (values[field.name] = e.currentTarget.value)}
					/>
				{/if}
			{/if}
			{#if field.help}<p class="text-xs text-fg-muted">{field.help}</p>{/if}
			{#if errors[field.name]}
				<p class="text-xs text-danger" role="alert">{errors[field.name]}</p>
			{/if}
		</div>
	{/each}
</div>
