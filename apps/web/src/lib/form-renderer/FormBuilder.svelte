<!--
	Edits a form's fields: add, change, reorder and remove. `fields` is the
	builder's model; the page turns it into the schema and UI hints.
-->
<script lang="ts">
	import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/svelte';
	import { Button } from '@usehuxley/ui';
	import { KINDS, hasOptions, nameFrom, problemsOf, type BuilderField } from './builder.ts';
	import type { FieldKind } from '@usehuxley/ui';

	let { fields = $bindable([]) }: { fields?: BuilderField[] } = $props();

	let problems = $derived(problemsOf(fields));
	const inputClass =
		'w-full rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg';

	function add() {
		const name = nameFrom(
			'field',
			fields.map((f) => f.name)
		);
		fields.push({
			name,
			label: '',
			kind: 'text',
			required: false,
			help: '',
			placeholder: '',
			options: [],
			showIf: null,
			lookup: null,
			extra: {}
		});
	}

	function move(i: number, by: number) {
		const j = i + by;
		if (j < 0 || j >= fields.length) return;
		[fields[i], fields[j]] = [fields[j], fields[i]];
	}

	// A label typed into a new field names it, until the name is edited.
	function labelled(i: number, label: string) {
		const field = fields[i];
		const generated = /^field(_\d+)?$/.test(field.name);
		field.label = label;
		if (generated && label.trim()) {
			field.name = nameFrom(
				label,
				fields.filter((_, k) => k !== i).map((f) => f.name)
			);
		}
	}
</script>

<div class="flex flex-col gap-3">
	{#each fields as field, i (i)}
		<fieldset
			class="flex flex-col gap-2 rounded-card border border-border p-3"
			aria-label={`Field ${i + 1}`}
		>
			<div class="grid gap-2 sm:grid-cols-2">
				<label class="flex flex-col gap-1 text-xs text-fg-muted">
					Label
					<input
						class={inputClass}
						value={field.label}
						oninput={(e) => labelled(i, e.currentTarget.value)}
						placeholder="e.g. Start date"
					/>
				</label>
				<label class="flex flex-col gap-1 text-xs text-fg-muted">
					Type
					<select
						class={inputClass}
						value={field.kind}
						onchange={(e) => (field.kind = e.currentTarget.value as FieldKind)}
					>
						{#each KINDS as kind (kind.value)}
							<option value={kind.value}>{kind.label}</option>
						{/each}
					</select>
				</label>
				<label class="flex flex-col gap-1 text-xs text-fg-muted">
					Name in workflows
					<input class="{inputClass} font-mono" bind:value={field.name} spellcheck="false" />
				</label>
				<label class="flex flex-col gap-1 text-xs text-fg-muted">
					Help
					<input class={inputClass} bind:value={field.help} />
				</label>
				{#if hasOptions(field.kind)}
					<label class="flex items-center gap-2 text-sm text-fg sm:col-span-2">
						<input
							type="checkbox"
							checked={field.lookup !== null}
							onchange={(e) =>
								(field.lookup = e.currentTarget.checked
									? { action: '', connection: '', items: '', value: '', label: '', extra: {} }
									: null)}
						/>
						Choices from an integration
					</label>
					{#if field.lookup}
						<label class="flex flex-col gap-1 text-xs text-fg-muted">
							Action (read-only)
							<input
								class="{inputClass} font-mono"
								bind:value={field.lookup.action}
								placeholder="msgraph/listUsers"
								spellcheck="false"
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-fg-muted">
							Connection (optional)
							<input class="{inputClass} font-mono" bind:value={field.lookup.connection} />
						</label>
						<label class="flex flex-col gap-1 text-xs text-fg-muted">
							List in the response
							<input
								class="{inputClass} font-mono"
								bind:value={field.lookup.items}
								placeholder="value"
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-fg-muted">
							Value, then label, of each item
							<span class="flex gap-1">
								<input
									class="{inputClass} font-mono"
									aria-label="Value of each item"
									bind:value={field.lookup.value}
									placeholder="id"
								/>
								<input
									class="{inputClass} font-mono"
									aria-label="Label of each item"
									bind:value={field.lookup.label}
									placeholder="displayName"
								/>
							</span>
						</label>
					{:else}
						<label class="flex flex-col gap-1 text-xs text-fg-muted sm:col-span-2">
							Choices, one per line
							<textarea
								class={inputClass}
								rows="3"
								value={field.options.join('\n')}
								oninput={(e) => (field.options = e.currentTarget.value.split('\n'))}></textarea>
						</label>
					{/if}
				{:else if field.kind !== 'boolean'}
					<label class="flex flex-col gap-1 text-xs text-fg-muted">
						Placeholder
						<input class={inputClass} bind:value={field.placeholder} />
					</label>
				{/if}
			</div>
			<div class="grid gap-2 sm:grid-cols-2">
				<label class="flex flex-col gap-1 text-xs text-fg-muted">
					Show only when
					<select
						class={inputClass}
						value={field.showIf?.field ?? ''}
						onchange={(e) => {
							const target = e.currentTarget.value;
							field.showIf = target ? { field: target, values: field.showIf?.values ?? '' } : null;
						}}
					>
						<option value="">Always shown</option>
						<!-- By position: names can repeat while they're being typed. -->
						{#each fields as other, k (k)}
							{#if k !== i}<option value={other.name}>{other.label || other.name}</option>{/if}
						{/each}
					</select>
				</label>
				{#if field.showIf}
					<label class="flex flex-col gap-1 text-xs text-fg-muted">
						…is (comma-separated; empty: answered)
						<input class={inputClass} bind:value={field.showIf.values} />
					</label>
				{/if}
			</div>
			{#if problems.has(i)}
				<p class="text-xs text-danger" role="alert">{problems.get(i)}</p>
			{/if}
			<div class="flex items-center justify-between gap-2">
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" bind:checked={field.required} />
					Required
				</label>
				<div class="flex gap-1">
					<Button
						type="button"
						variant="ghost"
						size="sm"
						aria-label="Move up"
						disabled={i === 0}
						onclick={() => move(i, -1)}
					>
						<ArrowUp size={14} />
					</Button>
					<Button
						type="button"
						variant="ghost"
						size="sm"
						aria-label="Move down"
						disabled={i === fields.length - 1}
						onclick={() => move(i, 1)}
					>
						<ArrowDown size={14} />
					</Button>
					<Button
						type="button"
						variant="ghost"
						size="sm"
						aria-label="Remove field"
						onclick={() => fields.splice(i, 1)}
					>
						<Trash2 size={14} />
					</Button>
				</div>
			</div>
		</fieldset>
	{/each}
	<div>
		<Button type="button" onclick={add}><Plus size={14} /> Add field</Button>
	</div>
</div>
