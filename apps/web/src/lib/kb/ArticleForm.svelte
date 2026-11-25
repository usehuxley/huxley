<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Field, Input, Select } from '@usehuxley/ui';
	import ArticleBody from './ArticleBody.svelte';

	type Option = { value: string; label: string };
	let {
		article,
		orgs,
		categories,
		errors = {},
		submitLabel
	}: {
		/** The article being changed; none for a new one. */
		article?: {
			orgId: string;
			title: string;
			body: string;
			categoryId?: string | null;
			visibility: string;
			tags: string[];
			version: number;
		};
		/** Where a new article can go. */
		orgs: Option[];
		categories: Option[];
		errors?: Record<string, string>;
		submitLabel: string;
	} = $props();

	// svelte-ignore state_referenced_locally
	let body = $state(article?.body ?? '');
	let tab: 'write' | 'preview' = $state('write');
	let preview = $state('');
	let previewing = $state(false);

	async function showPreview() {
		tab = 'preview';
		previewing = true;
		try {
			const res = await fetch('/knowledge/preview', {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ body })
			});
			preview = res.ok ? ((await res.json()) as { html: string }).html : '';
		} finally {
			previewing = false;
		}
	}

	const visibilities: Option[] = [
		{ value: 'internal', label: 'Internal: technicians only' },
		{ value: 'clients', label: "Clients: also the organization's portal contacts" },
		{ value: 'public', label: 'Public: anyone on the portal' }
	];
</script>

<form method="POST" action="?/save" use:enhance class="flex flex-col gap-4">
	{#if article}
		<input type="hidden" name="version" value={article.version} />
	{/if}
	<div class="grid gap-4 sm:grid-cols-2">
		<Field label="Title" for="kb-title" error={errors.title} class="sm:col-span-2">
			<Input id="kb-title" name="title" required maxlength={300} value={article?.title ?? ''} />
		</Field>
		{#if !article}
			<Field
				label="Organization"
				for="kb-org"
				hint="Your MSP for shared know-how, or a client for its own documentation."
				error={errors.orgId}
			>
				<Select id="kb-org" name="orgId" options={orgs} />
			</Field>
		{/if}
		<Field label="Category" for="kb-category" error={errors.categoryId}>
			<Select
				id="kb-category"
				name="categoryId"
				options={[{ value: '', label: 'None' }, ...categories]}
				value={article?.categoryId ?? ''}
			/>
		</Field>
		<Field label="Who can see it" for="kb-visibility" error={errors.visibility}>
			<Select
				id="kb-visibility"
				name="visibility"
				options={visibilities}
				value={article?.visibility ?? 'internal'}
			/>
		</Field>
		<Field label="Tags" for="kb-tags" hint="Separated by commas" error={errors.tags}>
			<Input id="kb-tags" name="tags" value={article?.tags.join(', ') ?? ''} />
		</Field>
	</div>

	<div class="flex flex-col gap-2">
		<div class="flex items-center justify-between">
			<span class="text-sm font-medium text-fg" id="kb-body-label">Text</span>
			<div class="flex gap-1" role="tablist" aria-label="Text">
				<button
					type="button"
					role="tab"
					aria-selected={tab === 'write'}
					class="rounded-control px-2 py-1 text-sm {tab === 'write'
						? 'bg-surface-sunken text-fg'
						: 'text-fg-muted'}"
					onclick={() => (tab = 'write')}
				>
					Write
				</button>
				<button
					type="button"
					role="tab"
					aria-selected={tab === 'preview'}
					class="rounded-control px-2 py-1 text-sm {tab === 'preview'
						? 'bg-surface-sunken text-fg'
						: 'text-fg-muted'}"
					onclick={showPreview}
				>
					Preview
				</button>
			</div>
		</div>
		<textarea
			id="kb-body"
			name="body"
			aria-labelledby="kb-body-label"
			rows="18"
			hidden={tab !== 'write'}
			bind:value={body}
			placeholder="Markdown: # headings, **bold**, `code`, lists, tables and [links](https://…)"
			class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 font-mono text-sm text-fg"
		></textarea>
		{#if tab === 'preview'}
			<div
				class="min-h-40 rounded-control border border-border bg-surface px-4 py-3"
				aria-busy={previewing}
			>
				{#if body.trim() === ''}
					<p class="text-sm text-fg-muted">Nothing to preview yet.</p>
				{:else}
					<ArticleBody html={preview} label="Preview" />
				{/if}
			</div>
		{/if}
		{#if errors.body}<p class="text-sm text-danger">{errors.body}</p>{/if}
	</div>

	<div><Button type="submit" variant="primary">{submitLabel}</Button></div>
</form>
