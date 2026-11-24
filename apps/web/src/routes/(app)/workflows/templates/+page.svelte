<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let categories = $derived([...new Set(data.templates.map((t) => t.category))]);
</script>

<svelte:head>
	<title>Templates · Workflows · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/workflows')}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} /> Workflows
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">Templates</h1>
		<p class="mt-1 text-sm text-fg-muted">
			Start from one: you fill in a few details, and get a draft to review and publish.
		</p>
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg" role="status">{form.message}</p>
	{/if}
	{#each categories as category (category)}
		<section class="flex flex-col gap-3" aria-label={category}>
			<h2 class="text-sm font-medium text-fg-muted">{category}</h2>
			<div class="grid gap-4 md:grid-cols-2">
				{#each data.templates.filter((t) => t.category === category) as template (template.key)}
					<a
						href={resolve('/(app)/workflows/templates/[key]', { key: template.key })}
						class="block rounded-card focus-visible:outline-2 focus-visible:outline-accent"
					>
						<Card title={template.name} description={template.description}>
							{#if template.hasForm}<Badge tone="info">Includes a form</Badge>{/if}
							{#if template.orgId}<Badge tone="neutral">Saved by your organization</Badge>{/if}
						</Card>
					</a>
					{#if template.orgId && data.canPublish}
						<form method="POST" action="?/delete" use:enhance class="-mt-2">
							<input type="hidden" name="id" value={template.key} />
							<Button type="submit" size="sm" variant="ghost">Delete {template.name}</Button>
						</form>
					{/if}
				{/each}
			</div>
		</section>
	{/each}
</div>
