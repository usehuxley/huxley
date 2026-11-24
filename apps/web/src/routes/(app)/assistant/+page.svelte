<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
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
	const submitting = () => {
		busy = true;
		return async ({ update }: { update: (o?: { reset?: boolean }) => Promise<void> }) => {
			await update();
			busy = false;
		};
	};
</script>

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
		{#if form && 'message' in form && form.message}
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

			{#if conversation.pending.length}
				<Card title="Waiting for your approval">
					<form
						method="POST"
						action="?/decide"
						use:enhance={submitting}
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
				<form method="POST" action="?/say" use:enhance={submitting} class="flex flex-col gap-2">
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
			<form method="POST" action="?/start" use:enhance={submitting} class="flex flex-col gap-2">
				<input type="hidden" name="orgId" value={data.orgId} />
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
