<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import type { PageProps } from './$types';

	type AssetType = components['schemas']['AssetType'];
	type FieldType = components['schemas']['AssetTypeField']['fieldType'];
	/** A field being edited; options as lines of text. */
	type Draft = {
		key: string;
		label: string;
		help: string;
		fieldType: FieldType;
		options: string;
		required: boolean;
	};

	let { data, form }: PageProps = $props();
	let open = $state(false);
	let editing: AssetType | null = $state(null);
	let name = $state('');
	let drafts: Draft[] = $state([]);
	let errors: Record<string, string> = $derived(form?.errors ?? {});

	const FIELD_TYPES: { value: FieldType; label: string }[] = [
		{ value: 'text', label: 'Text' },
		{ value: 'long_text', label: 'Long text' },
		{ value: 'number', label: 'Number' },
		{ value: 'boolean', label: 'Yes / no' },
		{ value: 'date', label: 'Date' },
		{ value: 'select', label: 'Choice' },
		{ value: 'multi_select', label: 'Choices' },
		{ value: 'url', label: 'URL' },
		{ value: 'email', label: 'Email' }
	];
	const typeLabel = (t: string) => FIELD_TYPES.find((f) => f.value === t)?.label ?? t;
	const hasOptions = (t: FieldType) => t === 'select' || t === 'multi_select';
	/** A key from a label: lowercase letters, digits and _. */
	const keyFrom = (label: string) =>
		label
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, '_')
			.replace(/^_+|_+$/g, '')
			.replace(/^(?=[0-9])/, 'f_')
			.slice(0, 63);

	function edit(t: AssetType | null) {
		editing = t;
		name = t?.name ?? '';
		drafts = (t?.fields ?? []).map((f) => ({
			key: f.key,
			label: f.label,
			help: f.help ?? '',
			fieldType: f.fieldType,
			options: (f.options ?? [])
				.map((o) => (o.value === keyFrom(o.label) ? o.label : `${o.value} | ${o.label}`))
				.join('\n'),
			required: f.required ?? false
		}));
		open = true;
	}

	/** The fields as the API takes them. */
	let fieldsJson = $derived(
		JSON.stringify(
			drafts.map((d) => ({
				key: d.key || keyFrom(d.label),
				label: d.label,
				help: d.help,
				fieldType: d.fieldType,
				required: d.required,
				options: hasOptions(d.fieldType)
					? d.options
							.split('\n')
							.map((line) => line.trim())
							.filter(Boolean)
							.map((line) => {
								const [value, label] = line.includes('|')
									? line.split('|').map((x) => x.trim())
									: [keyFrom(line), line];
								return { value, label };
							})
					: []
			}))
		)
	);

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
	<title>Asset types · Settings · Huxley</title>
</svelte:head>

<Card
	title="Asset types"
	description="Kinds of asset, each with its own fields. Built-in types can be extended, not removed."
>
	{#snippet actions()}
		<Button onclick={() => edit(null)}><Plus size={16} /> New type</Button>
	{/snippet}
	<ul class="flex flex-col divide-y divide-border" aria-label="Asset types">
		{#each data.types as t (t.id)}
			<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
				<div class="min-w-0">
					<div class="flex flex-wrap items-center gap-2">
						<span class="text-sm font-medium text-fg">{t.name}</span>
						{#if t.isSystem}<Badge>Built in</Badge>{/if}
						{#if t.archived}<Badge tone="neutral">Archived</Badge>{/if}
					</div>
					<p class="text-xs text-fg-subtle">
						{t.fields.length === 0 ? 'No fields' : t.fields.map((f) => f.label).join(', ')} · {t.assets}
						assets
					</p>
				</div>
				<div class="flex shrink-0 gap-2">
					<Button size="sm" onclick={() => edit(t)} aria-label="Edit {t.name}">Edit</Button>
					{#if !t.isSystem && !t.archived}
						<form method="POST" action="?/archive" use:enhance={done('Type archived.')}>
							<input type="hidden" name="id" value={t.id} />
							<Button size="sm" type="submit" variant="ghost">Archive</Button>
						</form>
					{/if}
				</div>
			</li>
		{/each}
	</ul>
</Card>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New asset type'}>
	<form
		method="POST"
		action="?/save"
		use:enhance={done('Type saved.', () => (open = false))}
		class="flex flex-col gap-4"
	>
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
			<input type="hidden" name="position" value={editing.position} />
		{/if}
		<input type="hidden" name="fields" value={fieldsJson} />
		<Field label="Name" for="type-name" error={errors.name}>
			<Input id="type-name" name="name" required maxlength={100} bind:value={name} />
		</Field>
		<fieldset class="flex flex-col gap-3">
			<legend class="mb-1 text-sm font-medium text-fg">Fields</legend>
			{#each drafts as d, i (i)}
				<div
					class="flex flex-col gap-2 rounded-control border border-border p-3"
					role="group"
					aria-label="Field {i + 1}"
				>
					<div class="grid gap-2 sm:grid-cols-[1fr_10rem_auto]">
						<Field label="Label" for="f-label-{i}" error={errors[`fields/${i}/label`]}>
							<Input id="f-label-{i}" bind:value={d.label} required maxlength={100} />
						</Field>
						<Field label="Kind" for="f-type-{i}">
							<Select id="f-type-{i}" options={FIELD_TYPES} bind:value={d.fieldType} />
						</Field>
						<div class="flex items-end pb-1">
							<button
								type="button"
								class="text-fg-subtle hover:text-danger"
								aria-label="Remove field {d.label || i + 1}"
								onclick={() => (drafts = drafts.filter((_, j) => j !== i))}
							>
								<Trash2 size={16} />
							</button>
						</div>
					</div>
					{#if hasOptions(d.fieldType)}
						<Field
							label="Options"
							for="f-options-{i}"
							hint="One a line; 'value | Label' to set the stored value"
							error={errors[`fields/${i}/options`]}
						>
							<textarea
								id="f-options-{i}"
								rows="3"
								bind:value={d.options}
								class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
							></textarea>
						</Field>
					{/if}
					<div class="flex flex-wrap items-center gap-4 text-sm">
						<label class="flex items-center gap-2 text-fg">
							<input type="checkbox" bind:checked={d.required} /> Required
						</label>
						<span class="text-xs text-fg-subtle">
							Kept as <code>{d.key || keyFrom(d.label) || '…'}</code>{d.key
								? ''
								: ' (from the label)'} · {typeLabel(d.fieldType)}
						</span>
					</div>
				</div>
			{/each}
			<div>
				<Button
					size="sm"
					variant="secondary"
					onclick={() =>
						(drafts = [
							...drafts,
							{ key: '', label: '', help: '', fieldType: 'text', options: '', required: false }
						])}
				>
					<Plus size={14} /> Add a field
				</Button>
			</div>
		</fieldset>
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
