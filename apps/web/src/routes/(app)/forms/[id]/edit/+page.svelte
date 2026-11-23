<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import FormBuilder from '#lib/form-renderer/FormBuilder.svelte';
	import FormRenderer from '#lib/form-renderer/FormRenderer.svelte';
	import {
		builderFieldsOf,
		definitionOf,
		problemsOf,
		type BuilderField
	} from '#lib/form-renderer/builder.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let mode = $state<'builder' | 'json'>('builder');
	let fields = $state<BuilderField[]>([]);
	let schemaText = $state('');
	let uiText = $state('');
	let switchProblem = $state('');
	// Start from what's saved (again after each save).
	$effect(() => {
		fields = builderFieldsOf(data.form.schema, data.form.ui);
		schemaText = JSON.stringify(data.form.schema, null, 2);
		uiText = JSON.stringify(data.form.ui, null, 2);
	});
	const tryParse = (text: string): unknown => {
		try {
			return JSON.parse(text);
		} catch {
			return undefined;
		}
	};
	let built = $derived(definitionOf(fields, data.form.ui));
	let problems = $derived(problemsOf(fields));
	// What gets saved and previewed: the builder's, or the JSON text.
	let schema = $derived(mode === 'builder' ? built.schema : tryParse(schemaText));
	let ui = $derived(mode === 'builder' ? built.ui : (tryParse(uiText) ?? {}));
	let preview = $state<Record<string, unknown>>({});
	const when = (v: string) => new Date(v).toLocaleString();
	// The preview looks choices up with the saved definition.
	const previewOptions = (name: string): string =>
		resolve('/(app)/forms/[id]/fields/[name]/options', { id: data.form.id, name });

	function toJson() {
		schemaText = JSON.stringify(built.schema, null, 2);
		uiText = JSON.stringify(built.ui, null, 2);
		switchProblem = '';
		mode = 'json';
	}

	function toBuilder() {
		const parsedSchema = tryParse(schemaText);
		const parsedUi = tryParse(uiText);
		if (parsedSchema === undefined || parsedUi === undefined) {
			switchProblem = 'Fix the JSON first.';
			return;
		}
		fields = builderFieldsOf(parsedSchema, parsedUi);
		switchProblem = '';
		mode = 'builder';
	}
</script>

<svelte:head>
	<title>Edit {data.form.name} · Forms · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/forms/[id]', { id: data.form.id })}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} />
			{data.form.name}
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">Edit form</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Key <span class="font-mono">{data.form.key}</span>: workflows listen for it with
			<span class="font-mono">{`{ type: form, form: ${data.form.key} }`}</span>.
		</p>
	</header>
	{#if form?.message}<p class="text-sm text-fg" role="status">{form.message}</p>{/if}
	<div class="grid gap-6 md:grid-cols-2">
		<Card
			title="Definition"
			description="The questions, or the JSON Schema of the answers and UI hints behind them."
		>
			<!-- Not reset after saving: the builder's inputs follow form resets. -->
			<form
				method="POST"
				action="?/save"
				use:enhance={() =>
					({ update }) =>
						update({ reset: false })}
				class="flex flex-col gap-3"
			>
				<input type="hidden" name="key" value={data.form.key} />
				<Field label="Name" for="name">
					<Input id="name" name="name" value={data.form.name} required maxlength={200} />
				</Field>
				<Field label="Description" for="description">
					<Input id="description" name="description" value={data.form.description ?? ''} />
				</Field>
				<div class="flex items-center gap-2 text-sm" role="group" aria-label="Editor">
					<Button
						type="button"
						size="sm"
						variant={mode === 'builder' ? 'primary' : 'ghost'}
						aria-pressed={mode === 'builder'}
						onclick={toBuilder}
					>
						Fields
					</Button>
					<Button
						type="button"
						size="sm"
						variant={mode === 'json' ? 'primary' : 'ghost'}
						aria-pressed={mode === 'json'}
						onclick={toJson}
					>
						JSON
					</Button>
					{#if switchProblem}<span class="text-xs text-danger">{switchProblem}</span>{/if}
				</div>
				{#if mode === 'builder'}
					<FormBuilder bind:fields />
					<input type="hidden" name="schema" value={JSON.stringify(built.schema)} />
					<input type="hidden" name="ui" value={JSON.stringify(built.ui)} />
				{:else}
					<label for="schema" class="text-sm font-medium text-fg">Schema</label>
					<textarea
						id="schema"
						name="schema"
						rows="14"
						spellcheck="false"
						class="rounded-control border border-border bg-surface p-2 font-mono text-xs text-fg"
						bind:value={schemaText}></textarea>
					<label for="ui" class="text-sm font-medium text-fg">UI hints</label>
					<textarea
						id="ui"
						name="ui"
						rows="5"
						spellcheck="false"
						class="rounded-control border border-border bg-surface p-2 font-mono text-xs text-fg"
						bind:value={uiText}></textarea>
					<p class="text-xs text-fg-muted">
						e.g. {`{ "order": ["name"], "notes": { "widget": "textarea", "help": "…" } }`}
					</p>
				{/if}
				<div>
					<Button type="submit" disabled={mode === 'builder' && problems.size > 0}>Save</Button>
				</div>
			</form>
			<form
				method="POST"
				action={data.form.archived ? '?/restore' : '?/archive'}
				use:enhance
				class="mt-3"
			>
				<Button type="submit" variant="ghost">{data.form.archived ? 'Restore' : 'Archive'}</Button>
			</form>
		</Card>
		<Card title="Preview">
			{#if schema === undefined}
				<p class="text-sm text-fg-muted">The schema isn't valid JSON yet.</p>
			{:else}
				<FormRenderer
					{schema}
					{ui}
					bind:values={preview}
					idPrefix="preview"
					optionsUrl={previewOptions}
				/>
			{/if}
		</Card>
	</div>
	<Card
		title="Public links"
		description="Anyone with a link can fill this form in without signing in. Their runs act as each workflow's run-as account."
	>
		{#if form && 'linkToken' in form && form.linkToken}
			<div class="mb-3 flex flex-col gap-1 text-sm" role="status">
				<p class="text-fg">The link, shown once:</p>
				<pre
					class="overflow-x-auto rounded-control bg-surface-sunken p-2 font-mono text-xs"
					data-testid="form-link">{data.origin}/f/{form.linkToken}</pre>
			</div>
		{/if}
		{#if data.links.length}
			<ul class="mb-3 flex flex-col divide-y divide-border text-sm" aria-label="Public links">
				{#each data.links as link (link.id)}
					<li class="flex items-center justify-between gap-3 py-2">
						<span class="min-w-0">
							<span class="text-fg">{link.name}</span>
							<span class="text-xs text-fg-muted">
								{#if link.revokedAt}· turned off{:else if link.expiresAt}· until {when(
										link.expiresAt
									)}{/if}
								· last used {link.lastUsedAt ? when(link.lastUsedAt) : 'never'}
							</span>
						</span>
						{#if !link.revokedAt}
							<form method="POST" action="?/revokeLink" use:enhance>
								<input type="hidden" name="linkId" value={link.id} />
								<Button type="submit" size="sm" variant="danger">Turn off</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		<form method="POST" action="?/link" use:enhance class="flex flex-wrap items-end gap-3">
			<Field label="Link name" for="link-name" hint="Who it's for, e.g. Contoso staff">
				<Input id="link-name" name="name" required maxlength={200} />
			</Field>
			<Field label="Expires (optional)" for="link-expires">
				<Input id="link-expires" name="expiresAt" type="datetime-local" />
			</Field>
			<Button type="submit" variant="secondary">Make a link</Button>
		</form>
	</Card>

	<Card title="Recent submissions">
		{#if data.submissions.length === 0}
			<p class="text-sm text-fg-muted">None yet.</p>
		{:else}
			<ul class="flex flex-col gap-2 text-sm" aria-label="Submissions">
				{#each data.submissions as s (s.id)}
					<li>
						<span class="text-fg">{when(s.createdAt)}</span>
						<span class="text-fg-muted">· {s.runIds.length} run(s)</span>
						{#each s.runIds as run (run)}
							<a
								href={resolve('/(app)/runs/[id]', { id: run })}
								class="ml-2 text-accent hover:underline"
							>
								run
							</a>
						{/each}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
