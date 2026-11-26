<script lang="ts">
	import { resolve } from '$app/paths';
	import { ArticleBody, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>{data.article.title} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div>
		<a href={resolve('/(portal)/knowledge')} class="text-sm text-accent hover:underline">
			Help articles
		</a>
		<h1 class="mt-1 text-2xl font-semibold text-fg">{data.article.title}</h1>
		{#if data.article.category}<p class="text-sm text-fg-muted">{data.article.category}</p>{/if}
	</div>
	<Card>
		<ArticleBody html={data.article.bodyHtml} label={data.article.title} />
	</Card>
	<p class="text-sm text-fg-muted">
		Still stuck?
		<a href={resolve('/(portal)/tickets/new')} class="text-accent hover:underline">
			Raise a ticket
		</a>
	</p>
</div>
