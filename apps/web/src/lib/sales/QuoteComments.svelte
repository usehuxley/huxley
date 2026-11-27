<!-- Comments on a quote or one of its lines: threads with replies,
     @mentions (who are told), resolved when settled. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { AtSign, MessageSquare } from '@lucide/svelte';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Comment = components['schemas']['Comment'];
	type Option = { value: string; label: string };

	let {
		comments,
		lines,
		people,
		me,
		done
	}: {
		comments: Comment[];
		lines: Option[];
		people: Option[];
		me: string;
		done: (message: string, after?: () => void) => SubmitFunction;
	} = $props();

	let threads = $derived(
		comments
			.filter((c) => !c.parentId)
			.map((c) => ({ ...c, replies: comments.filter((r) => r.parentId === c.id) }))
	);
	let open = $derived(threads.filter((t) => !t.resolvedAt));
	let settled = $derived(threads.filter((t) => t.resolvedAt));
	let showSettled = $state(false);

	let body = $state('');
	let lineId = $state('');
	let mentions = $state<string[]>([]);
	let replyTo = $state<string | null>(null);
	let reply = $state('');

	function mention(id: string) {
		const who = people.find((p) => p.value === id);
		if (!who || mentions.includes(id)) return;
		mentions.push(id);
		body = `${body}${body && !body.endsWith(' ') ? ' ' : ''}@${who.label} `;
	}
	const when = (v: string) => new Date(v).toLocaleString();
	const textarea =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg';
</script>

{#snippet comment(c: Comment)}
	<div id="comment-{c.id}">
		<p class="text-xs text-fg-subtle">
			<span class="font-semibold text-fg">{c.authorName ?? 'Someone'}</span>
			· {when(c.createdAt)}
		</p>
		<p class="mt-0.5 text-sm whitespace-pre-wrap text-fg">{c.body}</p>
	</div>
{/snippet}

{#snippet thread(t: (typeof threads)[number])}
	<li class="flex flex-col gap-2 rounded-control border border-border p-3">
		{#if t.lineName}
			<Badge>On {t.lineName}</Badge>
		{/if}
		{@render comment(t)}
		{#each t.replies as r (r.id)}
			<div class="border-l-2 border-border pl-3">{@render comment(r)}</div>
		{/each}
		{#if t.resolvedAt}
			<p class="text-xs text-fg-subtle">
				Resolved{t.resolvedByName ? ` by ${t.resolvedByName}` : ''} · {when(t.resolvedAt)}
			</p>
		{/if}
		{#if replyTo === t.id}
			<form
				method="POST"
				action="?/comment"
				class="flex flex-col gap-2"
				use:enhance={done('Replied.', () => ((reply = ''), (replyTo = null)))}
			>
				<input type="hidden" name="parentId" value={t.id} />
				<textarea
					name="body"
					rows="2"
					maxlength="4000"
					required
					aria-label="Reply"
					class={textarea}
					bind:value={reply}></textarea>
				<div class="flex gap-2">
					<Button type="submit" size="sm" variant="primary">Reply</Button>
					<Button size="sm" variant="ghost" onclick={() => (replyTo = null)}>Cancel</Button>
				</div>
			</form>
		{:else}
			<div class="flex gap-2">
				<Button size="sm" variant="ghost" onclick={() => ((replyTo = t.id), (reply = ''))}>
					Reply
				</Button>
				<form
					method="POST"
					action="?/resolve"
					use:enhance={done(t.resolvedAt ? 'Opened again.' : 'Resolved.')}
				>
					<input type="hidden" name="comment" value={t.id} />
					<input type="hidden" name="resolved" value={t.resolvedAt ? 'no' : 'yes'} />
					<Button type="submit" size="sm" variant="ghost">
						{t.resolvedAt ? 'Open again' : 'Resolve'}
					</Button>
				</form>
			</div>
		{/if}
	</li>
{/snippet}

<Card title="Comments">
	<div class="flex flex-col gap-4">
		{#if open.length > 0}
			<ul class="flex flex-col gap-3" aria-label="Open comments">
				{#each open as t (t.id)}{@render thread(t)}{/each}
			</ul>
		{:else}
			<p class="flex items-center gap-2 text-sm text-fg-muted">
				<MessageSquare size={15} aria-hidden="true" /> No open comments.
			</p>
		{/if}
		{#if settled.length > 0}
			<button
				type="button"
				class="self-start text-xs text-fg-muted hover:text-fg"
				onclick={() => (showSettled = !showSettled)}
				aria-expanded={showSettled}
			>
				{showSettled ? 'Hide' : 'Show'}
				{settled.length} resolved
			</button>
			{#if showSettled}
				<ul class="flex flex-col gap-3" aria-label="Resolved comments">
					{#each settled as t (t.id)}{@render thread(t)}{/each}
				</ul>
			{/if}
		{/if}

		<form
			method="POST"
			action="?/comment"
			class="flex flex-col gap-2 border-t border-border pt-3"
			use:enhance={done('Comment added.', () => ((body = ''), (mentions = []), (lineId = '')))}
		>
			<label for="comment-body" class="text-sm font-semibold text-fg">Add a comment</label>
			<textarea
				id="comment-body"
				name="body"
				rows="3"
				maxlength="4000"
				required
				class={textarea}
				bind:value={body}></textarea>
			{#each mentions as m (m)}<input type="hidden" name="mentions" value={m} />{/each}
			<div class="flex flex-wrap items-center gap-2">
				<label class="sr-only" for="comment-line">On</label>
				<select
					id="comment-line"
					name="lineId"
					bind:value={lineId}
					class="h-8 max-w-56 rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
				>
					<option value="">The whole quote</option>
					{#each lines as l (l.value)}<option value={l.value}>{l.label}</option>{/each}
				</select>
				<label class="sr-only" for="comment-mention">Mention someone</label>
				<span class="inline-flex items-center gap-1">
					<AtSign size={14} class="text-fg-subtle" aria-hidden="true" />
					<select
						id="comment-mention"
						class="h-8 max-w-48 rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
						value=""
						onchange={(e) => {
							mention(e.currentTarget.value);
							e.currentTarget.value = '';
						}}
					>
						<option value="">Mention someone…</option>
						{#each people.filter((p) => p.value !== me) as p (p.value)}
							<option value={p.value}>{p.label}</option>
						{/each}
					</select>
				</span>
				<Button type="submit" size="sm" variant="primary">Comment</Button>
			</div>
			{#if mentions.length > 0}
				<p class="text-xs text-fg-muted">
					Telling {mentions
						.map((m) => people.find((p) => p.value === m)?.label ?? 'someone')
						.join(', ')}.
				</p>
			{/if}
		</form>
	</div>
</Card>
