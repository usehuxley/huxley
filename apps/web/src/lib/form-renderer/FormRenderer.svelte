<!--
	Renders a form's fields from its JSON Schema and UI hints, with the
	answers in `values` and per-field `errors` (by field name).
-->
<script lang="ts">
	import { fieldsOf, shownFields, type Choice, type Field } from './fields.ts';

	let {
		schema,
		ui = {},
		values = $bindable({}),
		errors = {},
		idPrefix = 'field',
		optionsUrl
	}: {
		schema: unknown;
		ui?: unknown;
		values?: Record<string, unknown>;
		errors?: Record<string, string>;
		idPrefix?: string;
		/** Where a looked-up field's choices come from. */
		optionsUrl?: (name: string) => string;
	} = $props();

	let fields = $derived(fieldsOf(schema, ui));
	let shown = $derived(shownFields(fields, values));
	// Looked-up choices, by field (and the URL they came from).
	let loaded = $state<Record<string, { url: string; choices: Choice[] | null; problem?: string }>>(
		{}
	);

	$effect(() => {
		for (const field of fields) {
			if (!field.lookup || !optionsUrl) continue;
			const url = optionsUrl(field.name);
			if (loaded[field.name]?.url === url) continue;
			loaded[field.name] = { url, choices: null };
			void fetch(url)
				.then(async (r) => {
					if (!r.ok)
						throw new Error(
							r.status === 404 ? 'Save the form to load its choices.' : 'Couldn’t load the choices.'
						);
					return (await r.json()) as Choice[];
				})
				.then(
					(choices) => {
						if (loaded[field.name]?.url === url) loaded[field.name] = { url, choices };
					},
					(e: unknown) => {
						if (loaded[field.name]?.url === url) {
							loaded[field.name] = {
								url,
								choices: [],
								problem: e instanceof Error ? e.message : String(e)
							};
						}
					}
				);
		}
	});

	// A field that's hidden has no answer.
	$effect(() => {
		for (const field of fields) {
			if (!shown.has(field.name) && values[field.name] !== undefined) delete values[field.name];
		}
	});

	function choicesOf(field: Field): Choice[] {
		if (field.lookup) return loaded[field.name]?.choices ?? [];
		return field.options.map((o) => ({ value: o, label: o }));
	}
	const id = (f: Field) => `${idPrefix}-${f.name}`;
	const inputClass = 'rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg';

	function toggle(field: Field, option: string, on: boolean) {
		const current = Array.isArray(values[field.name]) ? (values[field.name] as string[]) : [];
		values[field.name] = on ? [...current, option] : current.filter((o) => o !== option);
	}
	const str = (v: unknown) => (typeof v === 'string' || typeof v === 'number' ? String(v) : '');
</script>

<div class="flex flex-col gap-4">
	{#each fields.filter((f) => shown.has(f.name)) as field (field.name)}
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
					{#each choicesOf(field) as choice (choice.value)}
						<label class="flex items-center gap-2 text-sm text-fg">
							<input
								type="checkbox"
								checked={Array.isArray(values[field.name]) &&
									(values[field.name] as string[]).includes(choice.value)}
								onchange={(e) => toggle(field, choice.value, e.currentTarget.checked)}
							/>
							{choice.label}
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
						<option value="">
							{field.lookup && loaded[field.name]?.choices === null ? 'Loading…' : 'Choose…'}
						</option>
						{#each choicesOf(field) as choice (choice.value)}<option value={choice.value}>
								{choice.label}
							</option>{/each}
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
						autocomplete={field.kind === 'secret' ? 'off' : undefined}
						type={field.kind === 'secret'
							? 'password'
							: field.kind === 'email'
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
			{#if loaded[field.name]?.problem}
				<p class="text-xs text-danger">{loaded[field.name]?.problem}</p>
			{/if}
			{#if errors[field.name]}
				<p class="text-xs text-danger" role="alert">{errors[field.name]}</p>
			{/if}
		</div>
	{/each}
</div>
