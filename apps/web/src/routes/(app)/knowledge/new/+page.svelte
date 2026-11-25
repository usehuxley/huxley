<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card } from '@usehuxley/ui';
	import ArticleForm from '#lib/kb/ArticleForm.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>New article · Knowledge · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/knowledge')}>Knowledge</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">New article</h1>
		<p class="mt-1 text-fg-muted">It starts as a draft: submit it for review when it's ready.</p>
	</header>
	{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
	<Card>
		<ArticleForm
			orgs={data.orgs}
			categories={data.categories}
			errors={form?.errors ?? {}}
			submitLabel="Save draft"
		/>
	</Card>
</div>
