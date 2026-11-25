<script lang="ts">
	import { resolve } from '$app/paths';
	import { Button, Card, EmptyState, Input } from '@usehuxley/ui';
	import { Sparkles } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const percent = (score: number) => `${Math.round(Math.max(0, score) * 100)}%`;
</script>

<svelte:head>
	<title>Articles by meaning · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<header>
		<p class="text-sm">
			<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/knowledge')}>Knowledge</a>
		</p>
		<h1 class="text-2xl font-semibold text-fg">Articles by meaning</h1>
		<p class="mt-1 text-fg-muted">
			Describe the problem in your own words: articles about the same thing are found even when they
			use other words.
		</p>
	</header>

	<form method="GET" class="flex gap-2" role="search">
		<label for="kb-meaning-q" class="sr-only">Describe what you're looking for</label>
		<Input
			id="kb-meaning-q"
			name="q"
			value={data.q}
			placeholder="e.g. a user can't sign in after changing their phone"
			class="w-full"
			minlength={2}
			maxlength={500}
			required
		/>
		<Button type="submit" variant="primary"><Sparkles size={16} /> Search</Button>
	</form>

	{#if data.message}
		<p class="text-sm text-fg-muted" role="status">{data.message}</p>
	{/if}

	{#if data.result && data.result.available && !data.message}
		<Card>
			{#if data.result.hits.length === 0}
				<EmptyState
					title="Nothing found"
					description="No articles have been embedded yet. The Embed articles workflow (from the Knowledge search by meaning pack) embeds published articles every few minutes."
				/>
			{:else}
				<ol class="flex flex-col divide-y divide-border" aria-label="Articles by meaning">
					{#each data.result.hits as hit (hit.id)}
						<li class="flex items-center justify-between gap-4 py-2.5">
							<div class="min-w-0">
								<a
									class="font-medium text-fg hover:underline"
									href={resolve('/(app)/knowledge/[id]', { id: hit.id })}
								>
									{hit.title}
								</a>
								<p class="text-sm text-fg-muted">
									{hit.orgName}{hit.categoryName ? ` · ${hit.categoryName}` : ''}
								</p>
							</div>
							<span class="shrink-0 text-sm text-fg-muted" title="How close in meaning">
								{percent(hit.score)}
							</span>
						</li>
					{/each}
				</ol>
			{/if}
		</Card>
	{/if}
</div>
