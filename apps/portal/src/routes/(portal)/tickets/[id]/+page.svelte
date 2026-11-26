<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field } from '@usehuxley/ui';
	import { statusTone, when } from '#lib/tickets/status.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let t = $derived(data.ticket);
</script>

<svelte:head>
	<title>{t.reference} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<header>
		<a href={resolve('/(portal)/tickets')} class="text-sm text-accent hover:underline">Tickets</a>
		<div class="mt-1 flex flex-wrap items-center gap-3">
			<h1 class="text-2xl font-semibold text-fg">{t.title}</h1>
			<Badge tone={statusTone(t.statusCategory)}>{t.statusName}</Badge>
		</div>
		<p class="mt-1 text-sm text-fg-muted">
			{t.reference} · raised {when(t.createdAt)}{t.requesterName ? ` by ${t.requesterName}` : ''}
		</p>
	</header>

	{#if data.raised}
		<p class="rounded-md border border-border bg-surface px-4 py-3 text-sm text-fg" role="status">
			Thanks: your ticket is raised. We’ll reply here and by email.
		</p>
	{/if}

	{#if t.description}
		<Card title="Details">
			<p class="text-sm whitespace-pre-wrap text-fg">{t.description}</p>
		</Card>
	{/if}

	<Card title="Conversation">
		{#if t.notes.length === 0}
			<p class="text-sm text-fg-muted">No replies yet.</p>
		{:else}
			<ol class="flex flex-col gap-4" aria-label="Conversation">
				{#each t.notes as n (n.id)}
					<li
						class="rounded-md border px-4 py-3 {n.fromSupport
							? 'border-border bg-surface-sunken'
							: 'border-border bg-surface'}"
					>
						<p class="text-xs text-fg-muted">
							{n.authorName ?? (n.fromSupport ? 'Support' : 'You')}{n.fromSupport
								? ' (support)'
								: ''}
							· {when(n.createdAt)}{n.resolution ? ' · resolution' : ''}
						</p>
						<p class="mt-1 text-sm whitespace-pre-wrap text-fg">{n.body}</p>
					</li>
				{/each}
			</ol>
		{/if}
		{#if t.canReply}
			<form method="POST" action="?/reply" class="mt-4 flex flex-col gap-3" use:enhance>
				<Field
					label="Reply"
					for="reply"
					hint={t.statusCategory === 'resolved' ? 'Replying reopens the ticket.' : undefined}
				>
					<textarea
						id="reply"
						name="body"
						rows="4"
						required
						class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
					></textarea>
				</Field>
				{#if form && 'message' in form}<p class="text-sm text-danger" role="alert">
						{form.message}
					</p>{/if}
				{#if form && 'sent' in form}<p class="text-sm text-success" role="status">
						Reply sent.
					</p>{/if}
				<div><Button type="submit" variant="primary">Send reply</Button></div>
			</form>
		{:else}
			<p class="mt-4 text-sm text-fg-muted">
				This ticket is closed. Raise a new one if you need more help.
			</p>
		{/if}
	</Card>
</div>
