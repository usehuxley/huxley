<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { History, Pencil } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, toast, ArticleBody } from '@usehuxley/ui';
	import { STATUS_LABELS, statusTone } from '#lib/kb/form.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let a = $derived(data.article);
	/** Writers see the working copy, or the published version beside it. */
	let showing = $state<'draft' | 'published'>('draft');
	let pending = $derived(a.canWrite && a.published && a.published.revision !== a.revision);
	let shown = $derived(
		showing === 'published' && a.published
			? { title: a.published.title, html: a.published.bodyHtml }
			: { title: a.title, html: a.bodyHtml }
	);
	let rejecting = $state(false);

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: true });
			if (result.type === 'success') {
				const ok = result.data as { message?: string } | undefined;
				toast.push({ title: ok?.message ?? 'Done.', tone: 'success' });
				rejecting = false;
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const when = (v: string) => new Date(v).toLocaleString();
	const comment = (details: unknown) => (details as { comment?: string } | null)?.comment ?? '';
	const KINDS: Record<string, string> = {
		created: 'wrote it',
		edited: 'changed it',
		submitted: 'submitted it for review',
		rejected: 'sent it back',
		published: 'published it',
		archived: 'archived it',
		restored: 'restored it'
	};
	const VISIBILITY: Record<string, string> = {
		internal: 'Internal',
		clients: "Clients' portal contacts",
		public: 'Public on the portal'
	};
</script>

<svelte:head>
	<title>{a.title} · Knowledge · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header class="flex flex-wrap items-start justify-between gap-4">
		<div class="min-w-0">
			<p class="text-sm">
				<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/knowledge')}>Knowledge</a>
			</p>
			<h1 class="text-2xl font-semibold break-words text-fg">{shown.title}</h1>
			<div class="mt-2 flex flex-wrap items-center gap-2 text-sm text-fg-muted">
				{#if a.canWrite}
					<Badge tone={statusTone(a.status)}>{STATUS_LABELS[a.status] ?? a.status}</Badge>
				{/if}
				<span>{a.orgName}</span>
				{#if a.categoryName}<span>· {a.categoryName}</span>{/if}
				<span>· {VISIBILITY[a.visibility] ?? a.visibility}</span>
				{#each a.tags as tag (tag)}<Badge>{tag}</Badge>{/each}
			</div>
		</div>
		{#if a.canWrite}
			<div class="flex flex-wrap gap-2">
				{#if a.status !== 'archived'}
					<Button href={resolve('/(app)/knowledge/[id]/edit', { id: a.id })} variant="secondary">
						<Pencil size={16} /> Edit
					</Button>
				{/if}
				<Button href={resolve('/(app)/knowledge/[id]/revisions', { id: a.id })} variant="ghost">
					<History size={16} /> Revisions
				</Button>
			</div>
		{/if}
	</header>

	{#if a.canWrite}
		<Card>
			<div class="flex flex-wrap items-center justify-between gap-3" data-testid="kb-review">
				<p class="text-sm text-fg">
					{#if a.status === 'draft'}
						{a.published
							? `A draft of changes (revision ${a.revision}); readers see revision ${a.published.revision}.`
							: 'A draft: only writers see it.'}
					{:else if a.status === 'in_review'}
						In review{a.reviewRequestedByName ? `, submitted by ${a.reviewRequestedByName}` : ''}.
					{:else if a.status === 'published'}
						Published: readers see revision {a.publishedRevision}.
					{:else}
						Archived: readers don't see it.
					{/if}
				</p>
				<div class="flex flex-wrap gap-2">
					{#if a.status === 'draft'}
						<form method="POST" action="?/submit" use:enhance={done}>
							<Button type="submit">Submit for review</Button>
						</form>
					{/if}
					{#if a.canPublish && (a.status === 'draft' || a.status === 'in_review')}
						<form method="POST" action="?/publish" use:enhance={done}>
							<Button type="submit" variant="primary">
								{a.status === 'in_review' ? 'Approve and publish' : 'Publish'}
							</Button>
						</form>
					{/if}
					{#if a.canPublish && a.status === 'in_review'}
						<Button variant="danger" onclick={() => (rejecting = true)}>Reject</Button>
					{/if}
					{#if a.canPublish && a.status !== 'archived'}
						<form method="POST" action="?/archive" use:enhance={done}>
							<Button type="submit" variant="ghost">Archive</Button>
						</form>
					{/if}
					{#if a.canPublish && a.status === 'archived'}
						<form method="POST" action="?/restore" use:enhance={done}>
							<Button type="submit">Restore</Button>
						</form>
					{/if}
				</div>
			</div>
		</Card>
	{/if}

	{#if pending}
		<div class="flex gap-1" role="tablist" aria-label="Version">
			<button
				type="button"
				role="tab"
				aria-selected={showing === 'draft'}
				class="rounded-control px-3 py-1.5 text-sm {showing === 'draft'
					? 'bg-surface-sunken text-fg'
					: 'text-fg-muted'}"
				onclick={() => (showing = 'draft')}
			>
				Draft (revision {a.revision})
			</button>
			<button
				type="button"
				role="tab"
				aria-selected={showing === 'published'}
				class="rounded-control px-3 py-1.5 text-sm {showing === 'published'
					? 'bg-surface-sunken text-fg'
					: 'text-fg-muted'}"
				onclick={() => (showing = 'published')}
			>
				Published (revision {a.published?.revision})
			</button>
		</div>
	{/if}

	<div class="grid gap-6 lg:grid-cols-[1fr_16rem]">
		<Card>
			{#if shown.html.trim() === ''}
				<p class="text-sm text-fg-muted">No text yet.</p>
			{:else}
				<ArticleBody html={shown.html} />
			{/if}
		</Card>
		<aside class="flex flex-col gap-4 text-sm">
			<Card title="Details">
				<dl class="flex flex-col gap-2">
					{#if a.publishedAt}
						<div>
							<dt class="text-fg-muted">Published</dt>
							<dd>{when(a.publishedAt)}</dd>
						</div>
					{/if}
					<div>
						<dt class="text-fg-muted">Updated</dt>
						<dd>{when(a.updatedAt)}{a.updatedByName ? ` by ${a.updatedByName}` : ''}</dd>
					</div>
					{#if a.createdByName}
						<div>
							<dt class="text-fg-muted">Written by</dt>
							<dd>{a.createdByName}</dd>
						</div>
					{/if}
				</dl>
			</Card>
			{#if data.tickets.length > 0}
				<Card title="Linked tickets">
					<ul class="flex flex-col gap-2" aria-label="Linked tickets">
						{#each data.tickets as t (t.id)}
							<li>
								<a
									class="text-fg hover:underline"
									href={resolve('/(app)/tickets/[id]', { id: t.id })}
								>
									<span class="font-mono text-xs text-fg-muted">{t.reference}</span>
									{t.title}
								</a>
								<p class="text-xs text-fg-subtle">{t.orgName} · {t.statusName}</p>
							</li>
						{/each}
					</ul>
				</Card>
			{/if}
			{#if a.activity.length > 0}
				<Card title="History">
					<ol class="flex flex-col gap-2" aria-label="History">
						{#each [...a.activity].reverse() as event, i (i)}
							<li>
								<p class="text-fg">
									{event.actorName ?? 'Huxley'}
									{KINDS[event.kind] ?? event.kind}
								</p>
								{#if event.kind === 'rejected' && comment(event.details)}
									<p class="mt-0.5 rounded-control bg-surface-sunken px-2 py-1 text-fg">
										“{comment(event.details)}”
									</p>
								{:else if event.kind === 'published' && comment(event.details)}
									<p class="mt-0.5 text-fg-muted">“{comment(event.details)}”</p>
								{/if}
								<p class="text-xs text-fg-subtle">{when(event.createdAt)}</p>
							</li>
						{/each}
					</ol>
				</Card>
			{/if}
		</aside>
	</div>
</div>

<Dialog bind:open={rejecting} title="Send it back">
	<form method="POST" action="?/reject" use:enhance={done} class="flex flex-col gap-4">
		<Field label="What needs changing" for="kb-reject">
			<textarea
				id="kb-reject"
				name="comment"
				required
				rows="4"
				maxlength="2000"
				class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
			></textarea>
		</Field>
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={() => (rejecting = false)}>Cancel</Button>
			<Button type="submit" variant="danger">Send back</Button>
		</div>
	</form>
</Dialog>
