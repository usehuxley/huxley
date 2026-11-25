<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card } from '@usehuxley/ui';
	import ArticleForm from '#lib/kb/ArticleForm.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Edit {data.article.title} · Knowledge · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a
				class="text-fg-muted hover:text-fg"
				href={resolve('/(app)/knowledge/[id]', { id: data.article.id })}
			>
				{data.article.title}
			</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">Edit article</h1>
		{#if data.article.publishedRevision}
			<p class="mt-1 text-fg-muted">
				Changes to the title or text are a new draft: readers keep the published version until it's
				published.
			</p>
		{/if}
	</header>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	<Card>
		<ArticleForm
			article={data.article}
			orgs={[]}
			categories={data.categories}
			errors={form?.errors ?? {}}
			submitLabel="Save"
		/>
	</Card>
</div>
