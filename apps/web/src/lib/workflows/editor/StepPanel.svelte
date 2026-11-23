<script lang="ts">
	import { Button } from '@usehuxley/ui';
	import {
		COMMON_FIELDS,
		getAt,
		getField,
		isStep,
		pathKey,
		setField,
		stepSpec,
		type FieldSpec,
		type Step
	} from './catalog.ts';
	import type { EditorContext } from './context.ts';
	import ExpressionInput from './ExpressionInput.svelte';
	import { namesFor } from './suggest.ts';

	let { editor }: { editor: EditorContext } = $props();

	let path = $derived(editor.selected);
	let step = $derived(path ? getAt(editor.document, path) : undefined);
	let parent = $derived(path ? getAt(editor.document, path.slice(0, -1)) : undefined);
	let index = $derived(path ? (path[path.length - 1] as number) : -1);
	let spec = $derived(isStep(step) ? stepSpec(step.type) : undefined);
	let problems = $derived(path ? editor.problems(pathKey(path)) : []);
	// What this step's expressions can read, for autocomplete.
	let names = $derived(path ? namesFor(editor.document, path) : []);
	let jsonErrors = $state<Record<string, string>>({});

	const fieldId = (f: FieldSpec) => `step-field-${f.path.replaceAll('.', '-')}`;

	function text(value: unknown): string {
		if (value === undefined || value === null) return '';
		return typeof value === 'string' ? value : JSON.stringify(value);
	}

	function json(value: unknown): string {
		if (value === undefined) return '';
		// Expressions read better without JSON's quotes.
		if (typeof value === 'string' && value.includes('{{')) return value;
		return JSON.stringify(value, null, 2);
	}

	function set(field: string, value: unknown) {
		if (!isStep(step)) return;
		setField(step, field, value);
		editor.changed();
	}

	function setJson(field: string, raw: string) {
		const trimmed = raw.trim();
		if (!trimmed) {
			set(field, undefined);
			delete jsonErrors[field];
			return;
		}
		// An expression (`{{ … }}`) or plain words are a string; anything else
		// that looks like JSON has to parse.
		if (trimmed.startsWith('{{') || !/^[[{"]/.test(trimmed)) {
			let value: unknown = trimmed;
			if (!trimmed.startsWith('{{')) {
				try {
					value = JSON.parse(trimmed);
				} catch {
					value = trimmed;
				}
			}
			set(field, value);
			delete jsonErrors[field];
			return;
		}
		try {
			set(field, JSON.parse(trimmed));
			delete jsonErrors[field];
		} catch {
			jsonErrors[field] = 'Not valid JSON';
		}
	}

	function move(by: number) {
		if (!Array.isArray(parent) || !path) return;
		const target = index + by;
		if (target < 0 || target >= parent.length) return;
		const moved: unknown = parent.splice(index, 1)[0];
		parent.splice(target, 0, moved);
		editor.select([...path.slice(0, -1), target]);
		editor.changed();
	}

	function remove() {
		if (!Array.isArray(parent)) return;
		parent.splice(index, 1);
		editor.select(null);
		editor.changed();
	}

	function addLane() {
		if (!isStep(step)) return;
		if (spec?.lanes === 'cases') {
			if (!Array.isArray(step.cases)) step.cases = [];
			(step.cases as unknown[]).push({ when: '', steps: [] });
		} else if (spec?.lanes === 'branches') {
			if (!Array.isArray(step.branches)) step.branches = [];
			const ids = new Set((step.branches as Step[]).map((b) => b.id));
			let n = (step.branches as unknown[]).length + 1;
			while (ids.has(`branch_${n}`)) n++;
			(step.branches as unknown[]).push({ id: `branch_${n}`, steps: [] });
		}
		editor.changed();
	}

	function removeLane(i: number) {
		if (!isStep(step)) return;
		const lanes = step[spec?.lanes ?? ''];
		if (Array.isArray(lanes)) lanes.splice(i, 1);
		editor.changed();
	}

	function setLane(i: number, field: string, raw: string) {
		if (!isStep(step)) return;
		const lanes = step[spec?.lanes ?? ''];
		const lane: unknown = Array.isArray(lanes) ? lanes[i] : undefined;
		if (!isStep(lane)) return;
		let value: unknown = raw;
		if (field === 'when') {
			try {
				value = JSON.parse(raw);
			} catch {
				value = raw;
			}
		}
		lane[field] = value;
		editor.changed();
	}
</script>

{#if isStep(step) && path}
	<div class="flex flex-col gap-4" aria-label="Step settings">
		<div>
			<p class="text-xs font-medium tracking-wide text-fg-muted uppercase">
				{spec?.label ?? String(step.type)}
			</p>
			{#if spec}<p class="text-sm text-fg-muted">{spec.description}</p>{/if}
		</div>

		{#if problems.length > 0}
			<ul class="rounded-control border border-warning/40 bg-warning/5 p-2 text-sm text-fg">
				{#each problems as problem, i (i)}<li>{problem}</li>{/each}
			</ul>
		{/if}

		<div class="flex flex-col gap-1">
			<label for="step-id" class="text-sm font-medium text-fg">Step id</label>
			<input
				id="step-id"
				class="rounded-control border border-border bg-surface px-2 py-1 font-mono text-sm text-fg"
				value={text(step.id)}
				readonly={editor.readonly}
				pattern="[a-z][a-z0-9_]*"
				onchange={(e) => set('id', e.currentTarget.value.trim())}
			/>
			<p class="text-xs text-fg-muted">
				Other steps read this one as <code>{`steps.${text(step.id)}.output`}</code>.
			</p>
		</div>

		{#each [...(spec?.fields ?? []), ...COMMON_FIELDS] as field (field.path)}
			{@const value = getField(step, field.path)}
			<div class="flex flex-col gap-1">
				{#if field.kind === 'boolean'}
					<label class="flex items-center gap-2 text-sm text-fg">
						<input
							type="checkbox"
							checked={value === true || (field.path === 'wait' && value === undefined)}
							disabled={editor.readonly}
							onchange={(e) =>
								set(
									field.path,
									e.currentTarget.checked ? true : field.path === 'wait' ? false : undefined
								)}
						/>
						{field.label}
					</label>
				{:else}
					<label for={fieldId(field)} class="text-sm font-medium text-fg">{field.label}</label>
					{#if field.kind === 'select'}
						<select
							id={fieldId(field)}
							class="rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg"
							value={text(value) || field.options?.[0]?.value}
							disabled={editor.readonly}
							onchange={(e) => set(field.path, e.currentTarget.value)}
						>
							{#each field.options ?? [] as option (option.value)}
								<option value={option.value}>{option.label}</option>
							{/each}
						</select>
					{:else if field.kind === 'json'}
						<textarea
							id={fieldId(field)}
							rows="3"
							spellcheck="false"
							class="rounded-control border border-border bg-surface-sunken p-2 font-mono text-xs text-fg"
							value={json(value)}
							readonly={editor.readonly}
							onchange={(e) => setJson(field.path, e.currentTarget.value)}></textarea>
						{#if jsonErrors[field.path]}
							<p class="text-xs text-danger" role="alert">{jsonErrors[field.path]}</p>
						{/if}
					{:else if field.kind === 'number'}
						<input
							id={fieldId(field)}
							type="number"
							class="rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg"
							value={typeof value === 'number' ? value : ''}
							readonly={editor.readonly}
							onchange={(e) =>
								set(
									field.path,
									e.currentTarget.value === '' ? undefined : Number(e.currentTarget.value)
								)}
						/>
					{:else if field.kind === 'expression'}
						<ExpressionInput
							id={fieldId(field)}
							value={text(value)}
							{names}
							readonly={editor.readonly}
							onchange={(v) => set(field.path, v)}
						/>
					{:else}
						<input
							id={fieldId(field)}
							class="rounded-control border border-border bg-surface px-2 py-1 text-sm text-fg"
							value={text(value)}
							readonly={editor.readonly}
							onchange={(e) => set(field.path, e.currentTarget.value)}
						/>
					{/if}
					{#if field.hint}<p class="text-xs text-fg-muted">{field.hint}</p>{/if}
				{/if}
			</div>
		{/each}

		{#if spec?.lanes}
			{@const lanes = Array.isArray(step[spec.lanes]) ? (step[spec.lanes] as Step[]) : []}
			<fieldset class="flex flex-col gap-2">
				<legend class="text-sm font-medium text-fg">
					{spec.lanes === 'cases' ? 'Cases' : 'Branches'}
				</legend>
				{#each lanes as lane, i (i)}
					<div class="flex items-center gap-2">
						<input
							aria-label={spec.lanes === 'cases' ? `Case ${i + 1} value` : `Branch ${i + 1} id`}
							class="flex-1 rounded-control border border-border bg-surface px-2 py-1 font-mono text-sm text-fg"
							value={spec.lanes === 'cases' ? text(lane.when) : text(lane.id)}
							readonly={editor.readonly}
							onchange={(e) =>
								setLane(i, spec.lanes === 'cases' ? 'when' : 'id', e.currentTarget.value)}
						/>
						{#if !editor.readonly}
							<Button type="button" variant="ghost" size="sm" onclick={() => removeLane(i)}>
								Remove
							</Button>
						{/if}
					</div>
				{/each}
				{#if !editor.readonly}
					<Button type="button" variant="secondary" size="sm" onclick={addLane}>
						Add {spec.lanes === 'cases' ? 'case' : 'branch'}
					</Button>
				{/if}
			</fieldset>
		{/if}

		{#if !editor.readonly}
			<div class="flex flex-wrap gap-2 border-t border-border pt-3">
				<Button
					type="button"
					variant="secondary"
					size="sm"
					onclick={() => move(-1)}
					disabled={index <= 0}
				>
					Move up
				</Button>
				<Button
					type="button"
					variant="secondary"
					size="sm"
					onclick={() => move(1)}
					disabled={!Array.isArray(parent) || index >= parent.length - 1}
				>
					Move down
				</Button>
				<Button type="button" variant="danger" size="sm" onclick={remove}>Delete step</Button>
			</div>
		{/if}
	</div>
{:else}
	<p class="text-sm text-fg-muted">Select a step to edit it, or add one with +.</p>
{/if}
