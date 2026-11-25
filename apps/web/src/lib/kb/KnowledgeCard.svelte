<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Link2, X } from '@lucide/svelte';
	import { Badge, Button, Card, Input, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { knowledge }: { knowledge: components['schemas']['TicketKnowledge'] } = $props();

	type Found = { id: string; title: string; status: string; orgName: string };
	let q = $state('');
	let found: Found[] = $state([]);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function find(query: string) {
		const res = await fetch(`/knowledge/find?q=${encodeURIComponent(query)}`);
		if (res.ok && q.trim() === query) {
			const linked = new Set(knowledge.linked.map((l) => l.id));
			found = ((await res.json()) as Found[]).filter((f) => !linked.has(f.id));
		}
	}

	function search() {
		clearTimeout(timer);
		const query = q.trim();
		if (query.length < 2) {
			found = [];
			return;
		}
		timer = setTimeout(() => void find(query), 250);
	}

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const message =
				(result.type === 'success' || result.type === 'failure'
					? (result.data as { knowledge?: { message?: string } } | undefined)?.knowledge?.message
					: undefined) ?? 'Done.';
			toast.push({ title: message, tone: result.type === 'success' ? 'success' : 'danger' });
			if (result.type === 'success') {
				q = '';
				found = [];
			}
		};
</script>

<Card title="Knowledge">
	<div class="flex flex-col gap-4 text-sm">
		{#if knowledge.linked.length > 0}
			<ul class="flex flex-col gap-2" aria-label="Linked articles">
				{#each knowledge.linked as a (a.id)}
					<li class="flex items-start justify-between gap-2">
						<div class="min-w-0">
							<a
								class="text-fg hover:underline"
								href={resolve('/(app)/knowledge/[id]', { id: a.id })}
							>
								{a.title}
							</a>
							{#if a.status !== 'published'}<Badge>
									{a.status === 'in_review' ? 'In review' : 'Draft'}
								</Badge>{/if}
						</div>
						{#if knowledge.canLink}
							<form method="POST" action="?/unlinkArticle" use:enhance={done}>
								<input type="hidden" name="articleId" value={a.id} />
								<button
									type="submit"
									class="text-fg-subtle hover:text-fg"
									aria-label="Unlink {a.title}"
								>
									<X size={14} />
								</button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<p class="text-fg-muted">No articles linked.</p>
		{/if}

		{#if knowledge.suggested.length > 0}
			<div>
				<p class="mb-1 text-xs font-medium tracking-wide text-fg-subtle uppercase">Suggested</p>
				<ul class="flex flex-col gap-2" aria-label="Suggested articles">
					{#each knowledge.suggested as a (a.id)}
						<li class="flex items-start justify-between gap-2">
							<div class="min-w-0">
								<a
									class="text-fg hover:underline"
									href={resolve('/(app)/knowledge/[id]', { id: a.id })}
								>
									{a.title}
								</a>
								{#if a.score != null}
									<p class="text-xs text-fg-muted" title="How close in meaning">
										About the same thing ({Math.round(a.score * 100)}%)
									</p>
								{/if}
								{#if a.snippet}
									<!-- eslint-disable svelte/no-at-html-tags -- ts_headline output: escaped text with <b> marks -->
									<p class="text-xs text-fg-muted [&_b]:font-semibold [&_b]:text-fg">
										{@html a.snippet}
									</p>
									<!-- eslint-enable svelte/no-at-html-tags -->
								{/if}
							</div>
							{#if knowledge.canLink}
								<form method="POST" action="?/linkArticle" use:enhance={done}>
									<input type="hidden" name="articleId" value={a.id} />
									<Button type="submit" size="sm" aria-label="Link {a.title}">
										<Link2 size={14} /> Link
									</Button>
								</form>
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/if}

		{#if knowledge.canLink}
			<div class="flex flex-col gap-2">
				<label for="kb-find" class="sr-only">Find an article to link</label>
				<Input
					id="kb-find"
					bind:value={q}
					oninput={search}
					placeholder="Find an article to link"
					autocomplete="off"
				/>
				{#if found.length > 0}
					<ul class="flex flex-col gap-1" aria-label="Articles found">
						{#each found as a (a.id)}
							<li class="flex items-center justify-between gap-2">
								<span class="min-w-0 truncate text-fg">{a.title}</span>
								<form method="POST" action="?/linkArticle" use:enhance={done}>
									<input type="hidden" name="articleId" value={a.id} />
									<Button type="submit" size="sm" aria-label="Link {a.title}">
										<Link2 size={14} /> Link
									</Button>
								</form>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}
	</div>
</Card>
