<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import { BookOpen, Plus, Search, Sparkles, Tags } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Input, Select } from '@usehuxley/ui';
	import { STATUS_LABELS, statusTone } from '#lib/kb/form.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const withParams = (changes: Record<string, string | number | undefined>) => {
		const params = new SvelteURLSearchParams(page.url.search);
		for (const [key, value] of Object.entries(changes)) {
			if (value === undefined || value === '') params.delete(key);
			else params.set(key, String(value));
		}
		const query = params.toString();
		return `${resolve('/(app)/knowledge')}${query ? `?${query}` : ''}`;
	};
	const when = (v: string) => new Date(v).toLocaleDateString();
</script>

<svelte:head>
	<title>Knowledge · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Knowledge</h1>
			<p class="mt-1 text-fg-muted">
				How-tos, fixes and client documentation, reviewed before they're published.
			</p>
		</div>
		<div class="flex gap-2">
			{#if data.canPublish}
				<Button href={resolve('/(app)/knowledge/categories')} variant="secondary">
					<Tags size={16} /> Categories
				</Button>
			{/if}
			{#if data.canWrite}
				<Button href={resolve('/(app)/knowledge/new')} variant="primary">
					<Plus size={16} /> New article
				</Button>
			{/if}
		</div>
	</header>

	<form method="GET" class="flex flex-wrap items-end gap-2" role="search">
		<label for="kb-q" class="sr-only">Search articles</label>
		<Input
			id="kb-q"
			name="q"
			value={data.q}
			placeholder="Search titles, tags and text"
			class="w-72"
		/>
		<div class="w-44">
			<label for="kb-category-filter" class="sr-only">Category</label>
			<Select
				id="kb-category-filter"
				name="category"
				value={data.categoryId}
				options={[
					{ value: '', label: 'All categories' },
					...data.categories.map((c) => ({ value: c.id, label: c.name }))
				]}
			/>
		</div>
		{#if data.canWrite}
			<div class="w-40">
				<label for="kb-status-filter" class="sr-only">Status</label>
				<Select
					id="kb-status-filter"
					name="status"
					value={data.status}
					options={[
						{ value: '', label: 'Any status' },
						...Object.entries(STATUS_LABELS).map(([value, label]) => ({ value, label }))
					]}
				/>
			</div>
		{/if}
		<Button type="submit"><Search size={16} /> Search</Button>
		<Button
			href={data.q
				? `${resolve('/(app)/knowledge/meaning')}?q=${encodeURIComponent(data.q)}`
				: resolve('/(app)/knowledge/meaning')}
			variant="ghost"
			title="Find articles about the same thing, in other words"
		>
			<Sparkles size={16} /> By meaning
		</Button>
	</form>

	<Card>
		{#if data.articles.length === 0}
			<EmptyState
				title={data.q || data.categoryId || data.status ? 'No articles match' : 'No articles yet'}
				description={data.canWrite
					? 'Write the first one: a fix you keep explaining is a good start.'
					: 'Nothing has been published here yet.'}
			>
				{#snippet icon()}<BookOpen size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Articles">
				{#each data.articles as a (a.id)}
					<li class="flex flex-col gap-1 py-3 first:pt-0 last:pb-0">
						<div class="flex flex-wrap items-center gap-2">
							<a
								class="font-medium text-fg hover:underline"
								href={resolve('/(app)/knowledge/[id]', { id: a.id })}
							>
								{a.title}
							</a>
							{#if data.canWrite}
								<Badge tone={statusTone(a.status)}>{STATUS_LABELS[a.status] ?? a.status}</Badge>
								{#if a.status === 'draft' && a.publishedRevision}
									<Badge>Published earlier</Badge>
								{/if}
							{/if}
						</div>
						{#if a.snippet}
							<!-- eslint-disable svelte/no-at-html-tags -- ts_headline output: escaped text with <b> marks -->
							<p class="text-sm text-fg-muted [&_b]:font-semibold [&_b]:text-fg">
								{@html a.snippet}
							</p>
							<!-- eslint-enable svelte/no-at-html-tags -->
						{/if}
						<p class="text-xs text-fg-subtle">
							{a.orgName}{a.categoryName ? ` · ${a.categoryName}` : ''}{a.tags.length
								? ` · ${a.tags.join(', ')}`
								: ''} · updated {when(a.updatedAt)}
						</p>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	{#if data.pages > 1}
		<div class="flex items-center justify-between text-sm text-fg-muted">
			<span>Page {data.page} of {data.pages} · {data.total} articles</span>
			<div class="flex gap-2">
				{#if data.page > 1}
					<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with a new query -->
					<Button href={withParams({ page: data.page - 1 })} size="sm">Previous</Button>
				{/if}
				{#if data.page < data.pages}
					<Button href={withParams({ page: data.page + 1 })} size="sm">Next</Button>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
				{/if}
			</div>
		</div>
	{/if}
</div>
