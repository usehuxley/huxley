<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus, Trash2 } from '@lucide/svelte';
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

	type Mapping = components['schemas']['AssetMapping'];
	type Result = components['schemas']['AssetSyncResult'];

	let { data, form }: PageProps = $props();

	const FIELDS = [
		{ key: 'name', label: 'Name' },
		{ key: 'hostname', label: 'Hostname' },
		{ key: 'serial_number', label: 'Serial number' },
		{ key: 'manufacturer', label: 'Manufacturer' },
		{ key: 'model', label: 'Model' },
		{ key: 'external_id', label: 'Id in the source' }
	] as const;
	type FieldKey = (typeof FIELDS)[number]['key'];
	const MATCH = [
		{ value: 'external_id', label: 'Id in the source' },
		{ value: 'serial_number', label: 'Serial number' },
		{ value: 'hostname', label: 'Hostname' },
		{ value: 'name', label: 'Name' }
	];

	let open = $state(false);
	let editing: Mapping | null = $state(null);
	let editingVersion: number | null = $state(null);
	let datasetId = $state('');
	let name = $state('');
	let source = $state('');
	let typeId = $state('');
	let matchKeys: { field: string; column: string }[] = $state([]);
	let fields: Record<FieldKey, string> = $state({
		name: '',
		hostname: '',
		serial_number: '',
		manufacturer: '',
		model: '',
		external_id: ''
	});
	let attributes: Record<string, string> = $state({});
	let conflict = $state('manual_wins');
	let createMissing = $state(true);
	let enabled = $state(true);
	let placeBy = $state<'row' | 'variable'>('row');
	let orgColumn = $state('');
	let orgVariable = $state('');
	let errors: Record<string, string> = $derived(
		form && 'errors' in form ? (form.errors ?? {}) : {}
	);
	let results: Record<string, Result> = $state({});

	let dataset = $derived(data.datasets.find((d) => d.id === datasetId));
	let columns = $derived([
		{ value: '', label: '—' },
		...(dataset?.columns ?? []).map((c) => ({ value: c, label: c }))
	]);
	let type = $derived(data.types.find((t) => t.id === typeId));

	function edit(m: Mapping | null) {
		editing = m;
		editingVersion = m?.version ?? null;
		datasetId = m?.datasetId ?? data.datasets[0]?.id ?? '';
		name = m?.name ?? '';
		source = m?.source ?? 'rmm';
		typeId =
			m?.typeId ?? data.types.find((t) => t.key === 'workstation')?.id ?? data.types[0]?.id ?? '';
		matchKeys = m ? m.matchKeys.map((k) => ({ ...k })) : [{ field: 'external_id', column: '' }];
		const f = m?.fields;
		fields = {
			name: f?.name ?? '',
			hostname: f?.hostname ?? '',
			serial_number: f?.serial_number ?? '',
			manufacturer: f?.manufacturer ?? '',
			model: f?.model ?? '',
			external_id: f?.external_id ?? ''
		};
		attributes = { ...(f?.attributes ?? {}) };
		conflict = m?.conflict ?? 'manual_wins';
		createMissing = m?.createMissing ?? true;
		enabled = m?.enabled ?? true;
		placeBy = m?.orgMatch ? 'variable' : 'row';
		orgColumn = m?.orgMatch?.column ?? '';
		orgVariable = m?.orgMatch?.variable ?? '';
		open = true;
	}

	let mappingJson = $derived(
		JSON.stringify({
			datasetId,
			name,
			source,
			typeId,
			matchKeys: matchKeys.filter((k) => k.column),
			fields: {
				...Object.fromEntries(Object.entries(fields).filter(([, c]) => c)),
				attributes: Object.fromEntries(
					Object.entries(attributes).filter(([k, c]) => c && type?.fields.some((f) => f.key === k))
				)
			},
			conflict,
			createMissing,
			enabled,
			...(placeBy === 'variable' ? { orgMatch: { column: orgColumn, variable: orgVariable } } : {}),
			...(editingVersion !== null ? { version: editingVersion } : {})
		})
	);

	const summary = (r: Result) =>
		`${r.rows} rows: ${r.created} new, ${r.updated} updated, ${r.unchanged} unchanged, ${r.skipped} skipped`;
	const when = (v: string) => new Date(v).toLocaleString();

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const ran = result.data as { result?: Result; ranId?: string } | undefined;
				if (ran?.result && ran.ranId) {
					results = { ...results, [ran.ranId]: ran.result };
					toast.push({
						title: `${ran.result.preview ? 'Preview' : 'Synced'}: ${summary(ran.result)}`,
						tone: 'success'
					});
				} else {
					toast.push({ title: message, tone: 'success' });
				}
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Asset sync · Settings · Huxley</title>
</svelte:head>

<Card
	title="Asset sync"
	description="Turn a dataset's rows (an RMM's devices, Microsoft 365's tenants) into assets of each row's organization, matched and kept up to date."
>
	{#snippet actions()}
		<Button onclick={() => edit(null)} disabled={data.datasets.length === 0}>
			<Plus size={16} /> New mapping
		</Button>
	{/snippet}
	{#if data.mappings.length === 0}
		<EmptyState
			title="No mappings yet"
			description={data.datasets.length === 0
				? 'Make a dataset first (Datasets), then map it to assets here.'
				: 'Map a dataset to assets to sync them.'}
		/>
	{:else}
		<ul class="flex flex-col divide-y divide-border" aria-label="Asset mappings">
			{#each data.mappings as m (m.id)}
				{@const last = results[m.id] ?? m.lastResult}
				<li class="flex flex-col gap-2 py-3 first:pt-0">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<div class="flex flex-wrap items-center gap-2">
							<span class="font-medium text-fg">{m.name}</span>
							<Badge>{m.datasetName} → {m.typeName}</Badge>
							<Badge tone="info">{m.source}</Badge>
							{#if !m.enabled}<Badge tone="neutral">Off</Badge>{/if}
						</div>
						<div class="flex flex-wrap gap-2">
							<form method="POST" action="?/run" use:enhance={done('Done.')}>
								<input type="hidden" name="id" value={m.id} />
								<input type="hidden" name="preview" value="true" />
								<Button size="sm" type="submit" variant="secondary" aria-label="Preview {m.name}">
									Preview
								</Button>
							</form>
							<form method="POST" action="?/run" use:enhance={done('Done.')}>
								<input type="hidden" name="id" value={m.id} />
								<Button size="sm" type="submit" aria-label="Sync {m.name} now">Sync now</Button>
							</form>
							<Button size="sm" variant="ghost" onclick={() => edit(m)} aria-label="Edit {m.name}">
								Edit
							</Button>
						</div>
					</div>
					{#if last}
						<div class="text-sm text-fg-muted" data-testid="sync-result">
							<p>
								{last.preview ? 'Preview' : 'Last sync'}{m.lastRunAt && !last.preview
									? ` (${when(m.lastRunAt)})`
									: ''}: {summary(last)}
							</p>
							{#if last.problems.length > 0}
								<ul class="mt-1 list-disc pl-5 text-xs">
									{#each last.problems as p (p.row)}<li>Row {p.row}: {p.reason}</li>{/each}
								</ul>
							{/if}
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</Card>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New mapping'}>
	<form
		method="POST"
		action="?/save"
		use:enhance={done('Mapping saved.', () => (open = false))}
		class="flex flex-col gap-4"
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<input type="hidden" name="mapping" value={mappingJson} />
		<div class="grid gap-3 sm:grid-cols-2">
			<Field label="Name" for="map-name" error={errors.name}>
				<Input id="map-name" bind:value={name} required maxlength={100} />
			</Field>
			<Field label="Dataset" for="map-dataset" error={errors.datasetId}>
				<Select
					id="map-dataset"
					bind:value={datasetId}
					options={data.datasets.map((d) => ({ value: d.id, label: d.name }))}
				/>
			</Field>
			<Field
				label="Source"
				for="map-source"
				hint="Its short name, e.g. rmm or m365"
				error={errors.source}
			>
				<Input id="map-source" bind:value={source} required maxlength={32} />
			</Field>
			<Field label="New assets are" for="map-type" error={errors.typeId}>
				<Select
					id="map-type"
					bind:value={typeId}
					options={data.types.map((t) => ({ value: t.id, label: t.name }))}
				/>
			</Field>
		</div>

		<fieldset class="flex flex-col gap-2">
			<legend class="mb-1 text-sm font-medium text-fg">Match existing assets by (in order)</legend>
			{#each matchKeys as k, i (i)}
				<div class="flex items-end gap-2" role="group" aria-label="Match key {i + 1}">
					<div class="w-44">
						<Field label="Asset's" for="mk-field-{i}">
							<Select id="mk-field-{i}" bind:value={k.field} options={MATCH} />
						</Field>
					</div>
					<div class="w-44">
						<Field label="Column" for="mk-col-{i}" error={errors[`matchKeys/${i}/column`]}>
							<Select id="mk-col-{i}" bind:value={k.column} options={columns} />
						</Field>
					</div>
					<button
						type="button"
						class="pb-2 text-fg-subtle hover:text-danger"
						aria-label="Remove match key {i + 1}"
						onclick={() => (matchKeys = matchKeys.filter((_, j) => j !== i))}
					>
						<Trash2 size={16} />
					</button>
				</div>
			{/each}
			<div>
				<Button
					size="sm"
					variant="secondary"
					onclick={() => (matchKeys = [...matchKeys, { field: 'serial_number', column: '' }])}
				>
					<Plus size={14} /> Add a key
				</Button>
			</div>
		</fieldset>

		<fieldset class="grid gap-3 sm:grid-cols-2">
			<legend class="mb-1 text-sm font-medium text-fg">Fill in from</legend>
			{#each FIELDS as f (f.key)}
				<Field label={f.label} for="mf-{f.key}" error={errors[`fields/${f.key}`]}>
					<Select id="mf-{f.key}" bind:value={fields[f.key]} options={columns} />
				</Field>
			{/each}
			{#each type?.fields ?? [] as tf (tf.key)}
				<Field
					label="{type?.name}: {tf.label}"
					for="ma-{tf.key}"
					error={errors[`fields/attributes/${tf.key}`]}
				>
					<Select
						id="ma-{tf.key}"
						value={attributes[tf.key] ?? ''}
						onchange={(e: Event) => {
							attributes = {
								...attributes,
								[tf.key]: (e.currentTarget as HTMLSelectElement).value
							};
						}}
						options={columns}
					/>
				</Field>
			{/each}
		</fieldset>

		<fieldset class="flex flex-col gap-2">
			<legend class="mb-1 text-sm font-medium text-fg">Each asset belongs to</legend>
			<Select
				id="map-place"
				aria-label="Each asset belongs to"
				bind:value={placeBy}
				options={[
					{ value: 'row', label: 'The organization the row was written for' },
					{ value: 'variable', label: 'The organization whose variable matches a column' }
				]}
			/>
			{#if placeBy === 'variable'}
				<div class="grid gap-3 sm:grid-cols-2">
					<Field label="Column" for="map-org-column" error={errors['orgMatch/column']}>
						<Select id="map-org-column" bind:value={orgColumn} options={columns} />
					</Field>
					<Field
						label="Organization variable"
						for="map-org-variable"
						hint="e.g. automate_client_id, set on each client"
						error={errors['orgMatch/variable']}
					>
						<Input id="map-org-variable" bind:value={orgVariable} maxlength={64} />
					</Field>
				</div>
			{/if}
		</fieldset>

		<Field label="When a value was changed by hand" for="map-conflict" error={errors.conflict}>
			<Select
				id="map-conflict"
				bind:value={conflict}
				options={[
					{ value: 'manual_wins', label: 'Keep it (the source fills only what nobody changed)' },
					{ value: 'source_wins', label: 'Replace it with the source’s value' }
				]}
			/>
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" bind:checked={createMissing} /> Create assets for rows that match none
		</label>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" bind:checked={enabled} /> On
		</label>
		<div class="flex justify-between gap-2">
			<div>
				{#if editing}<Button type="submit" variant="danger" formaction="?/delete">
						Delete
					</Button>{/if}
			</div>
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</div>
	</form>
</Dialog>
