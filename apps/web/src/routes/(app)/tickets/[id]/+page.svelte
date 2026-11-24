<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Link2, Lock, Mail, Merge, MessageSquare, Pencil, X } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { describe } from '#lib/tickets/activity.ts';
	import { clockLines } from '#lib/tickets/sla.ts';
	import {
		LEVELS,
		NOTE_KINDS,
		PRIORITIES,
		SOURCES,
		ago,
		categoryTone,
		priorityOf
	} from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let ticket = $derived(data.ticket);
	let canWrite = $derived(data.user.grantedPermissions.includes('ticket:write'));
	let priority = $derived(priorityOf(ticket.priority));
	let clocks = $derived(clockLines(ticket));
	let statuses = $derived(data.type?.statuses ?? []);
	let statusName = (key: string) => statuses.find((s) => s.key === key)?.name ?? key;
	let hasResolution = $derived(data.notes.some((n) => n.kind === 'resolution'));

	// Statuses this one may move to (all, when the type doesn't limit them).
	let nextStatuses = $derived.by(() => {
		const allowed = data.type?.transitions[ticket.statusKey];
		const limited = data.type && Object.keys(data.type.transitions).length > 0;
		return statuses.filter(
			(s) => s.key === ticket.statusKey || !limited || (allowed ?? []).includes(s.key)
		);
	});

	let status = $state('');
	let assignee = $state('');
	let noteKind = $state('public');
	let noteBox: HTMLTextAreaElement | undefined = $state();

	/** Puts a canned response in the note (after what's written) and its kind. */
	function insert(id: string) {
		const response = data.responses.find((r) => r.id === id);
		if (!response || !noteBox) return;
		const written = noteBox.value.trimEnd();
		noteBox.value = written ? `${written}\n\n${response.body}` : response.body;
		noteKind = response.kind;
		noteBox.focus();
	}
	let editing = $state(false);
	let merging = $state(false);
	const LINK_KINDS = [
		{ value: 'related', label: 'Related' },
		{ value: 'parent', label: 'Parent' },
		{ value: 'child', label: 'Child' }
	] as const;
	$effect(() => {
		status = data.ticket.statusKey;
		assignee = data.ticket.assigneeUserId
			? `user:${data.ticket.assigneeUserId}`
			: data.ticket.assigneeTeamId
				? `team:${data.ticket.assigneeTeamId}`
				: '';
	});
	let target = $derived(statuses.find((s) => s.key === status));
	let needsResolution = $derived(
		status !== ticket.statusKey && !hasResolution && !!target?.requires.includes('resolution')
	);

	let assigneeOptions = $derived([
		{ value: '', label: 'Unassigned' },
		...data.people.map((p) => ({ value: `user:${p.value}`, label: p.label })),
		...data.teams.map((t) => ({ value: `team:${t.value}`, label: `Team: ${t.label}` }))
	]);

	// Notes and changes, oldest first. Note entries in the activity are the
	// notes themselves, so they're shown once, as notes.
	type Entry =
		| { at: string; note: (typeof data.notes)[number] }
		| { at: string; change: (typeof data.activity)[number] };
	let timeline: Entry[] = $derived(
		[
			...data.notes.map((note) => ({ at: note.createdAt, note })),
			...data.activity.filter((a) => a.kind !== 'note').map((change) => ({ at: change.at, change }))
		].sort((a, b) => a.at.localeCompare(b.at))
	);

	function errorsOf(
		key: 'properties' | 'status' | 'note' | 'edit' | 'link' | 'merge'
	): Record<string, string> {
		const part = form?.[key] as { errors?: Record<string, string> } | undefined;
		return part?.errors ?? {};
	}

	const saved =
		(done: string, after?: () => void): SubmitFunction =>
		({ formElement }) =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: done, tone: 'success' });
				after?.();
				if (formElement.dataset.reset !== undefined) formElement.reset();
			} else if (result.type === 'failure') {
				const failed = result.data as Record<string, { message?: string }> | undefined;
				const message = Object.values(failed ?? {})[0]?.message;
				if (message) toast.push({ title: message, tone: 'danger' });
			}
		};

	const when = (iso: string) => new Date(iso).toLocaleString();
	const textarea =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg disabled:opacity-50';
</script>

<svelte:head>
	<title>{ticket.reference} {ticket.title} · Tickets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/tickets')} class="text-sm text-fg-muted hover:text-fg">← Tickets</a>
		<div class="mt-2 flex flex-wrap items-start justify-between gap-3">
			<div class="min-w-0">
				<p class="text-sm text-fg-muted tabular-nums">
					{ticket.reference} · {ticket.typeName} ·
					<a
						href={resolve('/(app)/clients/[id]', { id: ticket.orgId })}
						class="hover:text-fg hover:underline"
					>
						{ticket.orgName}
					</a>
				</p>
				<h1 class="mt-1 text-2xl font-semibold break-words text-fg">{ticket.title}</h1>
				<div class="mt-2 flex flex-wrap gap-2">
					<Badge tone={categoryTone(ticket.statusCategory)}>{ticket.statusName}</Badge>
					<Badge tone={priority.tone}>{priority.label}</Badge>
				</div>
			</div>
			{#if canWrite}
				<div class="flex gap-2">
					{#if ticket.assigneeUserId !== data.user.id}
						<form method="POST" action="?/assign" use:enhance={saved('Assigned to you.')}>
							<input type="hidden" name="version" value={ticket.version} />
							<input type="hidden" name="userId" value={data.user.id} />
							<Button type="submit">Assign to me</Button>
						</form>
					{/if}
					<Button onclick={() => (editing = true)}><Pencil size={16} /> Edit</Button>
					{#if !ticket.mergedIntoId && ticket.statusCategory !== 'closed'}
						<Button onclick={() => (merging = true)}><Merge size={16} /> Merge</Button>
					{/if}
				</div>
			{/if}
		</div>
	</div>

	{#if ticket.mergedIntoId}
		<div
			role="status"
			class="rounded-card border border-border bg-surface-sunken p-4 text-sm text-fg"
		>
			Merged into
			<a
				href={resolve('/(app)/tickets/[id]', { id: ticket.mergedIntoId })}
				class="font-medium text-accent hover:underline"
			>
				{ticket.mergedIntoReference}
			</a>. Its conversation continues there.
		</div>
	{/if}

	<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
		<div class="flex min-w-0 flex-col gap-6">
			<Card title="Description">
				{#if ticket.description}
					<p class="text-sm break-words whitespace-pre-wrap text-fg">{ticket.description}</p>
				{:else}
					<p class="text-sm text-fg-muted">No description.</p>
				{/if}
			</Card>

			<section aria-label="Conversation" class="flex flex-col gap-3">
				<h2 class="text-sm font-semibold text-fg">Conversation</h2>
				<ol class="flex flex-col gap-3">
					{#each timeline as entry ('note' in entry ? entry.note.id : entry.change.id)}
						{#if 'note' in entry}
							{@const note = entry.note}
							<li
								class="rounded-card border p-4 {note.kind === 'internal'
									? 'border-warning/40 bg-warning-soft/40'
									: note.kind === 'resolution'
										? 'border-success/40 bg-success-soft/40'
										: 'border-border bg-surface'}"
							>
								<div class="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
									<span class="font-medium text-fg">
										{note.authorName ?? note.authorContactName ?? note.authorEmail ?? 'Someone'}
									</span>
									{#if note.emailId}<Badge tone="info"><Mail size={12} /> By email</Badge>{/if}
									{#if note.kind === 'internal'}
										<Badge tone="warning"><Lock size={12} /> Internal</Badge>
									{:else if note.kind === 'resolution'}
										<Badge tone="success">Resolution</Badge>
									{/if}
									<time datetime={note.createdAt} title={when(note.createdAt)}>
										{ago(note.createdAt)}
									</time>
								</div>
								<p class="mt-2 text-sm break-words whitespace-pre-wrap text-fg">{note.body}</p>
								{#if note.emailedAt}
									<p class="mt-2 text-xs text-fg-muted">
										Emailed to {note.emailedTo} · {ago(note.emailedAt)}
									</p>
								{:else if note.emailError}
									<p class="mt-2 text-xs text-danger">
										Couldn’t email {note.emailedTo}: {note.emailError}
									</p>
								{/if}
							</li>
						{:else}
							{@const change = entry.change}
							<li class="flex flex-wrap gap-x-1 px-1 text-xs text-fg-muted">
								<span class="font-medium text-fg">{change.actorName ?? 'Huxley'}</span>
								<span>{describe(change.kind, change.details, statusName)}</span>
								<span>·</span>
								<time datetime={change.at} title={when(change.at)}>{ago(change.at)}</time>
							</li>
						{/if}
					{/each}
				</ol>

				{#if canWrite}
					<form
						method="POST"
						action="?/note"
						class="flex flex-col gap-3 rounded-card border border-border bg-surface p-4"
						data-reset
						use:enhance={saved(noteKind === 'internal' ? 'Note added.' : 'Reply added.')}
					>
						<fieldset class="flex gap-4">
							<legend class="sr-only">Kind of note</legend>
							{#each NOTE_KINDS as kind (kind.value)}
								<label class="flex items-center gap-2 text-sm font-medium text-fg">
									<input type="radio" name="kind" value={kind.value} bind:group={noteKind} />
									{kind.label}
								</label>
							{/each}
						</fieldset>
						{#if data.responses.length > 0}
							<label class="flex items-center gap-2 text-sm text-fg-muted">
								Insert a response
								<select
									class="h-8 rounded-control border border-border-strong bg-surface px-2 text-sm text-fg"
									value=""
									onchange={(e) => {
										insert(e.currentTarget.value);
										e.currentTarget.value = '';
									}}
								>
									<option value="">Choose…</option>
									{#each data.responses as r (r.id)}
										<option value={r.id}>
											{r.category ? `${r.category} · ${r.name}` : r.name}
										</option>
									{/each}
								</select>
							</label>
						{/if}
						<label for="note-body" class="sr-only">
							{noteKind === 'internal' ? 'Internal note' : 'Reply'}
						</label>
						<textarea
							bind:this={noteBox}
							id="note-body"
							name="body"
							rows="4"
							required
							class={textarea}
							placeholder={NOTE_KINDS.find((k) => k.value === noteKind)?.hint}></textarea>
						{#if errorsOf('note').body}
							<p class="text-sm text-danger">{errorsOf('note').body}</p>
						{/if}
						<div class="flex justify-end">
							<Button type="submit" variant="primary">
								{#if noteKind === 'internal'}<Lock size={16} /> Add note{:else}<MessageSquare
										size={16}
									/> Reply{/if}
							</Button>
						</div>
					</form>
				{/if}
			</section>
		</div>

		<aside class="flex flex-col gap-6">
			<Card title="Status">
				<form
					method="POST"
					action="?/status"
					class="flex flex-col gap-3"
					use:enhance={saved('Status changed.')}
				>
					<input type="hidden" name="version" value={ticket.version} />
					<Field label="Status" for="status" error={errorsOf('status').status}>
						<Select
							id="status"
							name="status"
							bind:value={status}
							disabled={!canWrite}
							options={nextStatuses.map((s) => ({ value: s.key, label: s.name }))}
						/>
					</Field>
					{#if needsResolution}
						<Field label="Resolution" for="resolution" error={errorsOf('status').resolution}>
							<textarea
								id="resolution"
								name="resolution"
								rows="3"
								required
								class={textarea}
								placeholder="How was it resolved?"></textarea>
						</Field>
					{/if}
					{#if canWrite}
						<Button type="submit" variant="primary" disabled={status === ticket.statusKey}>
							Change status
						</Button>
					{/if}
				</form>
			</Card>

			<Card title="Properties">
				<form
					method="POST"
					action="?/properties"
					class="flex flex-col gap-3"
					use:enhance={saved('Ticket saved.')}
				>
					<input type="hidden" name="version" value={ticket.version} />
					<Field label="Assignee" for="assignee" error={errorsOf('properties').assigneeUserId}>
						<Select
							id="assignee"
							name="assignee"
							bind:value={assignee}
							disabled={!canWrite}
							options={assigneeOptions}
						/>
					</Field>
					<Field label="Queue" for="queueId" error={errorsOf('properties').queueId}>
						<Select
							id="queueId"
							name="queueId"
							value={ticket.queueId ?? ''}
							disabled={!canWrite}
							options={[
								{ value: '', label: 'None' },
								...data.queues.map((q) => ({ value: q.id, label: q.name }))
							]}
						/>
					</Field>
					<Field label="Requester" for="requesterId" error={errorsOf('properties').requesterId}>
						<Select
							id="requesterId"
							name="requesterId"
							value={ticket.requesterId ?? ''}
							disabled={!canWrite}
							options={[
								{ value: '', label: 'None' },
								...data.contacts.map((c) => ({ value: c.id, label: c.name }))
							]}
						/>
					</Field>
					<div class="grid grid-cols-2 gap-3">
						<Field label="Impact" for="impact">
							<Select
								id="impact"
								name="impact"
								value={String(ticket.impact)}
								disabled={!canWrite}
								options={[...LEVELS]}
							/>
						</Field>
						<Field label="Urgency" for="urgency">
							<Select
								id="urgency"
								name="urgency"
								value={String(ticket.urgency)}
								disabled={!canWrite}
								options={[...LEVELS]}
							/>
						</Field>
					</div>
					<Field
						label="Priority"
						for="priority"
						hint={ticket.priorityOverridden
							? 'Chosen instead of impact and urgency.'
							: `Works out to ${priority.label}.`}
					>
						<Select
							id="priority"
							name="priority"
							value={ticket.priorityOverridden ? ticket.priority : ''}
							disabled={!canWrite}
							options={[
								{ value: '', label: 'From impact and urgency' },
								...PRIORITIES.map((p) => ({ value: p.value, label: p.label }))
							]}
						/>
					</Field>
					{#if canWrite}<Button type="submit">Save</Button>{/if}
				</form>
			</Card>

			{#if ticket.slaPolicy}
				<Card title="SLA" description={ticket.slaPolicy}>
					{#if clocks.length === 0}
						<p class="text-sm text-fg-muted">No targets for {priority.short}.</p>
					{:else}
						<dl class="flex flex-col gap-2 text-sm">
							{#each clocks as clock (clock.label)}
								<div class="flex items-center justify-between gap-3">
									<dt class="text-fg-muted">{clock.label}</dt>
									<dd><Badge tone={clock.tone}>{clock.text}</Badge></dd>
								</div>
							{/each}
						</dl>
					{/if}
				</Card>
			{/if}

			<Card title="Linked tickets">
				{#if data.links.length === 0}
					<p class="text-sm text-fg-muted">None.</p>
				{:else}
					<ul class="flex flex-col gap-2" aria-label="Linked tickets">
						{#each data.links as link (link.id)}
							<li class="flex items-start justify-between gap-2 text-sm">
								<div class="min-w-0">
									<p class="text-xs text-fg-muted">
										{LINK_KINDS.find((k) => k.value === link.kind)?.label ?? link.kind} ·
										{link.statusName}
									</p>
									<a
										href={resolve('/(app)/tickets/[id]', { id: link.id })}
										class="block truncate text-fg hover:text-accent"
									>
										{link.reference} · {link.title}
									</a>
								</div>
								{#if canWrite}
									<form method="POST" action="?/unlink" use:enhance={saved('Unlinked.')}>
										<input type="hidden" name="otherId" value={link.id} />
										<button
											type="submit"
											class="rounded-control p-1 text-fg-muted hover:bg-surface-sunken hover:text-fg"
											aria-label={`Unlink ${link.reference}`}
										>
											<X size={14} />
										</button>
									</form>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				{#if canWrite}
					<form
						method="POST"
						action="?/link"
						class="mt-3 flex flex-col gap-2 border-t border-border pt-3"
						data-reset
						use:enhance={saved('Linked.')}
					>
						<div class="flex gap-2">
							<Input
								name="reference"
								aria-label="Ticket to link"
								placeholder="e.g. T-1042"
								required
								class="min-w-0 flex-1"
							/>
							<Select
								name="kind"
								aria-label="Link as"
								value="related"
								options={LINK_KINDS.map(({ value, label }) => ({ value, label }))}
							/>
						</div>
						{#if errorsOf('link').reference || errorsOf('link').kind}
							<p class="text-sm text-danger">
								{errorsOf('link').reference ?? errorsOf('link').kind}
							</p>
						{/if}
						<Button type="submit" size="sm"><Link2 size={14} /> Link</Button>
					</form>
				{/if}
			</Card>

			<Card title="Details">
				<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
					<dt class="text-fg-muted">Requester</dt>
					<dd class="min-w-0 truncate">
						{#if ticket.requesterId}
							<a
								href={resolve('/(app)/contacts/[id]', { id: ticket.requesterId })}
								class="text-accent hover:underline"
							>
								{ticket.requesterName ?? 'Contact'}
							</a>
							{#if ticket.requesterEmail}<span class="block truncate text-fg-muted">
									{ticket.requesterEmail}
								</span>{/if}
						{:else}
							<span class="text-fg-muted">None</span>
						{/if}
					</dd>
					<dt class="text-fg-muted">Received by</dt>
					<dd>{SOURCES.find((s) => s.value === ticket.source)?.label ?? ticket.source}</dd>
					<dt class="text-fg-muted">Created</dt>
					<dd title={when(ticket.createdAt)}>{ago(ticket.createdAt)}</dd>
					<dt class="text-fg-muted">First response</dt>
					<dd>
						{#if ticket.firstResponseAt}<span title={when(ticket.firstResponseAt)}>
								{ago(ticket.firstResponseAt)}
							</span>{:else}<span class="text-fg-muted">Not yet</span>{/if}
					</dd>
					{#if ticket.resolvedAt}
						<dt class="text-fg-muted">Resolved</dt>
						<dd title={when(ticket.resolvedAt)}>{ago(ticket.resolvedAt)}</dd>
					{/if}
					{#if ticket.closedAt}
						<dt class="text-fg-muted">Closed</dt>
						<dd title={when(ticket.closedAt)}>{ago(ticket.closedAt)}</dd>
					{/if}
				</dl>
			</Card>
		</aside>
	</div>
</div>

<Dialog bind:open={editing} title="Edit ticket">
	<form
		method="POST"
		action="?/edit"
		class="flex flex-col gap-4"
		use:enhance={saved('Ticket saved.', () => (editing = false))}
	>
		<input type="hidden" name="version" value={ticket.version} />
		<Field label="Title" for="edit-title" error={errorsOf('edit').title}>
			<Input id="edit-title" name="title" required maxlength={300} value={ticket.title} />
		</Field>
		<Field label="Description" for="edit-description" error={errorsOf('edit').description}>
			<textarea id="edit-description" name="description" rows="8" class={textarea}
				>{ticket.description}</textarea
			>
		</Field>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (editing = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>

<Dialog bind:open={merging} title={`Merge ${ticket.reference} into another ticket`}>
	<form method="POST" action="?/merge" class="flex flex-col gap-4" use:enhance={saved('Merged.')}>
		<p class="text-sm text-fg-muted">
			Its notes, attachments and emails move to the other ticket of {ticket.orgName}, and this one
			is closed. Replies to it will go there too.
		</p>
		<Field label="Merge into" for="merge-into" error={errorsOf('merge').into}>
			<Input id="merge-into" name="into" required placeholder="e.g. T-1042" />
		</Field>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (merging = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Merge</Button>
		</div>
	</form>
</Dialog>
