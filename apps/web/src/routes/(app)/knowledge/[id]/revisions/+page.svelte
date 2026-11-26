<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge, Card, ArticleBody } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const when = (v: string) => new Date(v).toLocaleString();
	const href = (n: number) =>
		`${resolve('/(app)/knowledge/[id]/revisions', { id: data.article.id })}?n=${n}`;
</script>

<svelte:head>
	<title>Revisions · {data.article.title} · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a
				class="text-fg-muted hover:text-fg"
				href={resolve('/(app)/knowledge/[id]', { id: data.article.id })}
			>
				{data.article.title}
			</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">Revisions</h1>
	</header>
	<div class="grid gap-6 lg:grid-cols-[16rem_1fr]">
		<Card>
			<ol class="flex flex-col gap-1" aria-label="Revisions">
				{#each data.revisions as r (r.number)}
					<li>
						<!-- eslint-disable svelte/no-navigation-without-resolve -- this page with ?n= -->
						<a
							href={href(r.number)}
							aria-current={data.revision?.number === r.number ? 'page' : undefined}
							class="block rounded-control px-2 py-1.5 text-sm {data.revision?.number === r.number
								? 'bg-surface-sunken text-fg'
								: 'text-fg-muted hover:text-fg'}"
						>
							<span class="flex items-center gap-2">
								Revision {r.number}
								{#if r.number === data.article.publishedRevision}<Badge tone="success">
										Published
									</Badge>{/if}
							</span>
							<span class="block text-xs text-fg-subtle">
								{when(r.createdAt)}{r.createdByName ? ` · ${r.createdByName}` : ''}
							</span>
						</a>
						<!-- eslint-enable svelte/no-navigation-without-resolve -->
					</li>
				{/each}
			</ol>
		</Card>
		{#if data.revision}
			<Card title={data.revision.title}>
				<ArticleBody html={data.revision.bodyHtml ?? ''} label="Revision {data.revision.number}" />
			</Card>
		{/if}
	</div>
</div>
