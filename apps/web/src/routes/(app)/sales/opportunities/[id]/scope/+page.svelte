<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onDestroy } from 'svelte';
	import { Check, FileText, Sparkles, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let s = $derived(data.scope);
	let template = $derived(data.templates.find((t) => t.key === s?.template));
	let guidance = $derived(
		Object.fromEntries((template?.sections ?? []).map((x) => [x.key, x.guidance]))
	);
	let open = $derived(s?.questions.filter((q) => !q.resolvedAt) ?? []);
	let waiting = $derived(s?.draftState === 'requested');

	// While a draft is on its way, look again every few seconds.
	let timer: ReturnType<typeof setInterval> | undefined;
	$effect(() => {
		if (waiting && !timer) timer = setInterval(() => void invalidateAll(), 3000);
		if (!waiting && timer) {
			clearInterval(timer);
			timer = undefined;
		}
	});
	onDestroy(() => timer && clearInterval(timer));

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: result.type === 'success' });
			const said = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (said as { message?: string } | undefined)?.message;
			if (result.type === 'success') {
				toast.push({ title: message ?? 'Saved.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				toast.push({ title: message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const area =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg disabled:bg-transparent';
</script>

<svelte:head>
	<title>Scope · {data.opportunity.name} · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-7xl flex-col gap-5">
	<div>
		<a
			href={resolve('/(app)/sales/opportunities/[id]', { id: data.opportunity.id })}
			class="text-sm text-fg-muted hover:text-fg"
		>
			← {data.opportunity.name}
		</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div>
				<p class="font-mono text-[12.5px] text-fg-subtle">{data.opportunity.orgName}</p>
				<h1 class="mt-1 text-2xl font-extrabold tracking-tight text-fg">Scope of work</h1>
				{#if s}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<Badge>{template?.name ?? s.template}</Badge>
						{#if s.versions.length}<Badge tone="success">
								Approved v{s.versions[0].number}
							</Badge>{/if}
						{#if waiting}<Badge tone="info">Drafting with AI…</Badge>{/if}
						{#if s.draftState === 'failed'}<Badge tone="danger">Drafting failed</Badge>{/if}
					</div>
				{/if}
			</div>
			{#if s && data.canWrite}
				<div class="flex flex-wrap gap-2">
					<form method="POST" action="?/draft" use:enhance={done()}>
						{#if waiting}
							<input type="hidden" name="cancel" value="yes" />
							<Button type="submit" variant="ghost">Stop waiting</Button>
						{:else}
							<Button
								type="submit"
								disabled={s.sources.length === 0}
								title={s.sources.length === 0 ? 'Add discovery first' : undefined}
							>
								<Sparkles size={16} /> Draft with AI
							</Button>
						{/if}
					</form>
					<form method="POST" action="?/approve" use:enhance={done()}>
						<input type="hidden" name="version" value={s.version} />
						<Button type="submit" variant="primary"><Check size={16} /> Approve</Button>
					</form>
				</div>
			{/if}
		</div>
		{#if s?.draftState === 'failed' && s.draftError}
			<p class="mt-3 rounded-control bg-danger-soft px-3 py-2 text-sm text-fg">{s.draftError}</p>
		{:else if waiting}
			<p class="mt-3 text-sm text-fg-subtle">
				The Presales scoping pack is writing the sections nobody has edited. If nothing happens,
				install the pack (Settings → Packs) and publish its workflow.
			</p>
		{/if}
	</div>

	{#if !s}
		{#if data.canWrite}
			<Card title="Start scoping">
				<form
					method="POST"
					action="?/open"
					class="flex flex-wrap items-end gap-3"
					use:enhance={done()}
				>
					<Field label="Kind of engagement" for="scope-template">
						<Select
							id="scope-template"
							name="template"
							options={data.templates.map((t) => ({ value: t.key, label: t.name }))}
						/>
					</Field>
					<Button type="submit" variant="primary">Start</Button>
				</form>
				<ul class="mt-4 flex flex-col gap-1 text-sm text-fg-muted">
					{#each data.templates as t (t.key)}<li>
							<strong class="text-fg">{t.name}:</strong>
							{t.description}
						</li>{/each}
				</ul>
			</Card>
		{:else}
			<EmptyState
				title="Not scoped yet"
				description="Nobody has started scoping this opportunity."
			/>
		{/if}
	{:else}
		<div class="grid gap-5 lg:grid-cols-[22rem_minmax(0,1fr)]">
			<div class="flex flex-col gap-5">
				<Card title="Discovery">
					<ul class="flex flex-col gap-2" aria-label="Discovery">
						{#each s.sources as src (src.id)}
							<li class="rounded-control border border-border p-2.5 text-sm">
								<div class="flex items-start justify-between gap-2">
									<p class="flex items-center gap-1.5 font-semibold text-fg">
										<FileText size={14} class="text-fg-subtle" aria-hidden="true" />
										{src.title}
									</p>
									{#if data.canWrite}
										<form method="POST" action="?/removeSource" use:enhance={done()}>
											<input type="hidden" name="source" value={src.id} />
											<button
												type="submit"
												class="text-fg-subtle hover:text-danger"
												aria-label="Remove {src.title}"
											>
												<Trash2 size={14} />
											</button>
										</form>
									{/if}
								</div>
								<p class="text-xs text-fg-subtle">{src.kind}</p>
								<p class="mt-1 line-clamp-3 text-xs whitespace-pre-wrap text-fg-muted">
									{src.body}
								</p>
							</li>
						{:else}
							<li class="text-sm text-fg-muted">
								Add meeting notes, a call transcript, or a document's text (an RFP, a site survey).
							</li>
						{/each}
					</ul>
					{#if data.canWrite}
						<form
							method="POST"
							action="?/source"
							class="mt-3 flex flex-col gap-2 border-t border-border pt-3"
							use:enhance={done()}
						>
							<div class="grid grid-cols-[7rem_minmax(0,1fr)] gap-2">
								<label class="sr-only" for="src-kind">Kind</label>
								<select
									id="src-kind"
									name="kind"
									class="h-9 rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
								>
									<option value="note">Note</option>
									<option value="transcript">Transcript</option>
									<option value="document">Document</option>
								</select>
								<Input
									name="title"
									required
									maxlength={200}
									placeholder="Title"
									aria-label="Title"
								/>
							</div>
							<textarea
								name="body"
								rows="4"
								required
								maxlength="100000"
								class={area}
								aria-label="Text"
								placeholder="Paste the text"></textarea>
							<Button type="submit" size="sm">Add to discovery</Button>
						</form>
					{/if}
				</Card>

				<Card title="Open questions">
					{#if s.questions.length === 0}
						<p class="text-sm text-fg-muted">
							Questions for the client show here: the AI's, and yours.
						</p>
					{:else}
						<ul class="flex flex-col gap-3" aria-label="Open questions">
							{#each s.questions as q (q.id)}
								<li class="text-sm">
									<p class="font-medium {q.resolvedAt ? 'text-fg-subtle line-through' : 'text-fg'}">
										{q.question}
									</p>
									<p class="text-xs text-fg-subtle">
										{q.sectionKey
											? s.sections.find((x) => x.key === q.sectionKey)?.title
											: 'General'}{q.fromAi ? ' · from the AI draft' : ''}
									</p>
									{#if data.canWrite}
										<form
											method="POST"
											action="?/answer"
											class="mt-1 flex gap-1"
											use:enhance={done()}
										>
											<input type="hidden" name="question" value={q.id} />
											<Input
												name="answer"
												value={q.answer ?? ''}
												placeholder="Answer"
												aria-label="Answer to {q.question}"
												class="flex-1"
											/>
											<Button
												type="submit"
												size="sm"
												name="resolved"
												value={q.resolvedAt ? 'no' : 'yes'}
											>
												{q.resolvedAt ? 'Reopen' : 'Resolve'}
											</Button>
										</form>
									{:else if q.answer}
										<p class="mt-1 text-fg-muted">{q.answer}</p>
									{/if}
								</li>
							{/each}
						</ul>
					{/if}
					{#if data.canWrite}
						<form
							method="POST"
							action="?/question"
							class="mt-3 flex gap-2 border-t border-border pt-3"
							use:enhance={done()}
						>
							<Input
								name="question"
								required
								maxlength={1000}
								placeholder="Ask the client…"
								aria-label="New question"
								class="flex-1"
							/>
							<Button type="submit" size="sm">Add</Button>
						</form>
					{/if}
					{#if open.length}<p class="mt-2 text-xs text-fg-subtle">{open.length} still open.</p>{/if}
				</Card>

				{#if s.versions.length}
					<Card title="Approved versions">
						<ol class="flex flex-col gap-1 text-sm" aria-label="Approved versions">
							{#each s.versions as v (v.id)}
								<li>
									<span class="font-semibold text-fg">Version {v.number}</span>
									<span class="text-xs text-fg-subtle">
										{v.createdByName ?? ''} · {new Date(v.createdAt).toLocaleString()}
									</span>
								</li>
							{/each}
						</ol>
						<p class="mt-2 text-xs text-fg-subtle">
							Attach a version to a quote: it's signed with it.
						</p>
					</Card>
				{/if}
			</div>

			<div class="flex flex-col gap-4">
				{#each s.sections as x (x.key)}
					<Card title={x.title}>
						<form method="POST" action="?/section" class="flex flex-col gap-2" use:enhance={done()}>
							<input type="hidden" name="key" value={x.key} />
							<p class="text-xs text-fg-subtle">{guidance[x.key] ?? ''}</p>
							<textarea
								name="body"
								rows={Math.min(14, Math.max(4, x.body.split('\n').length + 1))}
								maxlength="20000"
								class={area}
								aria-label={x.title}
								disabled={!data.canWrite}>{x.body}</textarea
							>
							<div class="flex flex-wrap items-center justify-between gap-2">
								<div class="flex flex-wrap gap-1">
									{#if x.drafted && x.body}<Badge tone="info">AI draft</Badge>{/if}
									{#each x.citations as c, i (i)}<Badge>{c.label}</Badge>{/each}
								</div>
								{#if data.canWrite}<Button type="submit" size="sm">Save</Button>{/if}
							</div>
						</form>
					</Card>
				{/each}
			</div>
		</div>
	{/if}
</div>
