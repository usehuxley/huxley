<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import { formString } from '#lib/forms.ts';
	import { readEvents } from '#lib/sse.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let busy = $state(false);
	let conversation = $derived(data.conversation);
	const labels: Record<string, string> = {
		create_workflow_draft: 'Create a workflow draft',
		update_workflow_draft: 'Change a workflow draft',
		test_workflow_document: 'Test-run a workflow (unsaved)',
		test_run_workflow: 'Test-run a workflow'
	};
	const label = (name: string) => labels[name] ?? name.replaceAll('_', ' ');
	// An answer as it streams in: the question, then text and tool calls in
	// the order they happen. Replaced by the saved conversation at the end.
	type LiveItem =
		| { kind: 'text'; text: string }
		| { kind: 'tool'; id: string; name: string; state: 'running' | 'done' | 'failed' };
	type Problem = { title?: string; detail?: string; message?: string } | null;
	let live = $state<{ question: string | null; items: LiveItem[]; turnOpen: boolean } | null>(null);
	let streamError = $state<string | null>(null);
	const problemText = (p: Problem) =>
		p?.detail ?? p?.message ?? p?.title ?? 'The assistant could not answer.';

	/** Streams an answer; true once the conversation is saved and shown. */
	async function stream(
		action: 'start' | 'say' | 'decide',
		payload: Record<string, unknown>,
		question: string | null
	): Promise<boolean> {
		busy = true;
		streamError = null;
		live = { question, items: [], turnOpen: false };
		try {
			const res = await fetch(resolve('/(app)/assistant/stream'), {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ action, ...payload })
			});
			if (!res.ok || !res.body) {
				streamError = problemText((await res.json().catch(() => null)) as Problem);
				return false;
			}
			for await (const { event, data } of readEvents(res.body)) {
				const state = live;
				if (event === 'text') {
					const text = (data as { text: string }).text;
					const last = state.items.at(-1);
					if (state.turnOpen && last?.kind === 'text') last.text += text;
					else state.items.push({ kind: 'text', text });
					state.turnOpen = true;
				} else if (event === 'turn_ended') {
					state.turnOpen = false;
				} else if (event === 'tool_started') {
					const tool = data as { id: string; name: string };
					state.items.push({ kind: 'tool', id: tool.id, name: tool.name, state: 'running' });
				} else if (event === 'tool_finished') {
					const done = data as { id: string; isError: boolean };
					for (const item of state.items) {
						if (item.kind === 'tool' && item.id === done.id) {
							item.state = done.isError ? 'failed' : 'done';
						}
					}
				} else if (event === 'done') {
					const { id } = (data as { conversation: { id: string } }).conversation;
					if (action === 'start') {
						// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string
						await goto(`${resolve('/(app)/assistant')}?c=${id}`, { invalidateAll: true });
					} else {
						await invalidateAll();
					}
					return true;
				} else if (event === 'failed') {
					streamError = problemText((data as { problem: Problem }).problem);
					await invalidateAll();
					return false;
				}
			}
			streamError = 'The answer stopped early. Reload to see what was saved.';
			return false;
		} catch {
			streamError = 'The connection was lost. Reload to see what was saved.';
			return false;
		} finally {
			busy = false;
			live = null;
		}
	}

	/** Forms stream with JavaScript; without it they post as usual. */
	const streamed =
		(action: 'start' | 'say' | 'decide'): SubmitFunction =>
		({ formElement, formData, cancel }) => {
			cancel();
			const id = formString(formData, 'id');
			if (action === 'decide') {
				const decisions: Record<string, boolean> = {};
				for (const [key, value] of formData.entries()) {
					if (key.startsWith('call:')) decisions[key.slice(5)] = value === 'approve';
				}
				void stream('decide', { id, decisions }, null);
				return;
			}
			const text = formString(formData, 'text').trim();
			const contextId = formString(formData, 'contextId');
			const context = contextId
				? { kind: formString(formData, 'contextKind'), id: contextId }
				: null;
			void stream(action, { id, orgId: formString(formData, 'orgId'), text, context }, text).then(
				(ok) => {
					if (ok) formElement.reset();
				}
			);
		};
</script>

{#snippet answering()}
	{#if live}
		<ol class="flex flex-col gap-3" aria-label="Answer in progress" aria-busy="true">
			{#if live.question}
				<li
					class="self-end rounded-card bg-accent-soft px-3 py-2 text-sm whitespace-pre-wrap text-fg"
				>
					{live.question}
				</li>
			{/if}
			{#each live.items as item, i (i)}
				{#if item.kind === 'text'}
					<li class="text-sm whitespace-pre-wrap text-fg">{item.text}</li>
				{:else}
					<li class="text-xs text-fg-muted">
						{label(item.name)}:
						{item.state === 'running' ? 'running…' : item.state === 'done' ? 'done' : 'failed'}
					</li>
				{/if}
			{/each}
			{#if live.items.length === 0}
				<li class="text-sm text-fg-muted">Thinking…</li>
			{/if}
		</ol>
	{/if}
	<p class="sr-only" role="status">{live ? 'The assistant is answering…' : ''}</p>
{/snippet}

<svelte:head>
	<title>Assistant · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6 lg:flex-row">
	<aside class="flex shrink-0 flex-col gap-2 lg:w-64">
		<a href={resolve('/(app)/assistant')} class="text-sm font-medium hover:underline">
			New conversation
		</a>
		<ul class="flex flex-col gap-1 text-sm" aria-label="Conversations">
			{#each data.conversations as c (c.id)}
				<li>
					<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
					<a
						href={`${resolve('/(app)/assistant')}?c=${c.id}`}
						class="block truncate rounded-control px-2 py-1 hover:bg-surface-sunken {conversation?.id ===
						c.id
							? 'bg-surface-sunken font-medium'
							: ''}"
					>
						{c.title}
					</a>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
				</li>
			{/each}
		</ul>
	</aside>

	<div class="flex min-w-0 flex-1 flex-col gap-4">
		<header>
			<h1 class="text-2xl font-semibold text-fg">Assistant</h1>
			<p class="mt-1 text-sm text-fg-muted">
				Build and debug workflows, or ask about your setup. It acts with your permissions, saves
				only drafts, and asks before changing anything.
			</p>
		</header>
		{#if streamError}
			<p class="text-sm text-danger" role="alert">{streamError}</p>
		{:else if form && 'message' in form && form.message}
			<p class="text-sm text-danger" role="alert">{form.message}</p>
		{/if}

		{#if conversation}
			<ol class="flex flex-col gap-3" aria-label="Messages">
				{#each conversation.messages as m, i (i)}
					{#if m.role === 'user'}
						<li
							class="self-end rounded-card bg-accent-soft px-3 py-2 text-sm whitespace-pre-wrap text-fg"
						>
							{m.text}
						</li>
					{:else if m.role === 'assistant'}
						<li class="flex flex-col gap-2 text-sm">
							{#if m.text}<p class="whitespace-pre-wrap text-fg">{m.text}</p>{/if}
							{#each m.toolCalls ?? [] as call (call.id)}
								<details class="rounded-control border border-border px-2 py-1 text-xs">
									<summary class="cursor-pointer text-fg-muted">
										{label(call.name)}
										{#if call.needsApproval}<Badge tone="warning">needs approval</Badge>{/if}
									</summary>
									<pre class="mt-1 overflow-x-auto font-mono">{JSON.stringify(
											call.input,
											null,
											2
										)}</pre>
								</details>
							{/each}
						</li>
					{:else}
						<li class="text-xs">
							<details class="rounded-control bg-surface-sunken px-2 py-1">
								<summary class="cursor-pointer {m.isError ? 'text-danger' : 'text-fg-muted'}">
									{m.isError ? 'Tool failed' : 'Tool result'}
								</summary>
								<pre class="mt-1 overflow-x-auto font-mono whitespace-pre-wrap">{m.text}</pre>
							</details>
						</li>
					{/if}
				{/each}
			</ol>
			{@render answering()}

			{#if conversation.pending.length && !live}
				<Card title="Waiting for your approval">
					<form
						method="POST"
						action="?/decide"
						use:enhance={streamed('decide')}
						class="flex flex-col gap-3"
					>
						<input type="hidden" name="id" value={conversation.id} />
						{#each conversation.pending as call (call.id)}
							<fieldset class="flex flex-col gap-1 text-sm" data-testid="pending-call">
								<legend class="font-medium text-fg">{label(call.name)}</legend>
								<pre
									class="max-h-48 overflow-auto rounded-control bg-surface-sunken p-2 font-mono text-xs">{JSON.stringify(
										call.input,
										null,
										2
									)}</pre>
								<div class="flex gap-4">
									<label class="flex items-center gap-1.5">
										<input type="radio" name="call:{call.id}" value="approve" required /> Approve
									</label>
									<label class="flex items-center gap-1.5">
										<input type="radio" name="call:{call.id}" value="deny" /> Deny
									</label>
								</div>
							</fieldset>
						{/each}
						<div>
							<Button type="submit" disabled={busy}>{busy ? 'Working…' : 'Continue'}</Button>
						</div>
					</form>
				</Card>
			{:else}
				<form
					method="POST"
					action="?/say"
					use:enhance={streamed('say')}
					class="flex flex-col gap-2"
				>
					<input type="hidden" name="id" value={conversation.id} />
					<label for="say" class="sr-only">Message</label>
					<textarea
						id="say"
						name="text"
						rows={3}
						required
						maxlength={20000}
						placeholder="Ask a follow-up…"
						class="rounded-control border border-border bg-surface p-2 text-sm"></textarea>
					<div class="flex items-center justify-between">
						<Button type="submit" disabled={busy}>{busy ? 'Thinking…' : 'Send'}</Button>
					</div>
				</form>
				<form method="POST" action="?/delete" use:enhance>
					<input type="hidden" name="id" value={conversation.id} />
					<Button type="submit" variant="ghost" size="sm">Delete conversation</Button>
				</form>
			{/if}
		{:else}
			{@render answering()}
			<form
				method="POST"
				action="?/start"
				use:enhance={streamed('start')}
				class="flex flex-col gap-2"
			>
				<input type="hidden" name="orgId" value={data.orgId} />
				{#if data.context}
					<input type="hidden" name="contextKind" value={data.context.kind} />
					<input type="hidden" name="contextId" value={data.context.id} />
					<p class="text-sm text-fg-muted" role="note">
						About the {data.context.kind}: <strong class="text-fg">{data.context.label}</strong>
					</p>
				{/if}
				<label for="start" class="sr-only">Message</label>
				<textarea
					id="start"
					name="text"
					rows={4}
					required
					maxlength={20000}
					placeholder="e.g. Build a workflow that emails the team when a form is submitted"
					class="rounded-control border border-border bg-surface p-2 text-sm"></textarea>
				<div><Button type="submit" disabled={busy}>{busy ? 'Thinking…' : 'Ask'}</Button></div>
			</form>
		{/if}
	</div>
</div>
