<script lang="ts">
	import { resolve } from '$app/paths';
	import { BookOpen } from '@lucide/svelte';
	import { Button, Card, EmptyState, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Help articles · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Help articles</h1>
		<p class="mt-1 text-fg-muted">Answers to common questions, and how-tos.</p>
	</div>
	<form method="GET" class="flex gap-2" role="search">
		{#if data.category}<input type="hidden" name="category" value={data.category} />{/if}
		<Input name="q" value={data.q} placeholder="Search help articles" aria-label="Search" />
		<Button type="submit">Search</Button>
	</form>
	{#if data.categories.length > 0}
		<nav aria-label="Categories" class="flex flex-wrap gap-2 text-sm">
			<a
				href={resolve('/(portal)/knowledge')}
				class="rounded-full border px-3 py-1 {data.category
					? 'border-border text-fg-muted'
					: 'border-accent text-fg'}"
			>
				All
			</a>
			{#each data.categories as c (c.id)}
				<a
					href="{resolve('/(portal)/knowledge')}?category={c.id}"
					class="rounded-full border px-3 py-1 {data.category === c.id
						? 'border-accent text-fg'
						: 'border-border text-fg-muted'}"
				>
					{c.name} ({c.articles})
				</a>
			{/each}
		</nav>
	{/if}
	<Card>
		{#if data.articles.length === 0}
			<EmptyState
				title={data.q ? 'Nothing matches' : 'No articles yet'}
				description={data.q
					? 'Try other words, or raise a ticket.'
					: 'Raise a ticket if you need help.'}
			>
				{#snippet icon()}<BookOpen size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Articles">
				{#each data.articles as a (a.id)}
					<li class="py-3 first:pt-0">
						<a
							href={resolve('/(portal)/knowledge/[id]', { id: a.id })}
							class="font-medium text-fg hover:underline"
						>
							{a.title}
						</a>
						{#if a.category}<span class="ml-2 text-xs text-fg-muted">{a.category}</span>{/if}
						{#if a.snippet}
							<!-- eslint-disable-next-line svelte/no-at-html-tags -- escaped by the database's ts_headline over the article's markdown, with only <b> added -->
							<p class="mt-1 text-sm text-fg-muted">{@html a.snippet}</p>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
