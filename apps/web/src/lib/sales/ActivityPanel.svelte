<!-- What's been done and what's to do on a deal or lead: tasks, calls,
     meetings, notes, and emails logged from a sales mailbox. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { CalendarDays, CheckSquare, Mail, Phone, StickyNote } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Activity = components['schemas']['SalesActivity'];
	let {
		activities,
		people,
		canWrite
	}: {
		activities: Activity[];
		people: { value: string; label: string }[];
		canWrite: boolean;
	} = $props();

	let kind = $state('task');
	let adding = $state(false);
	let open = $derived(activities.filter((a) => a.kind === 'task' && !a.doneAt));
	let past = $derived(activities.filter((a) => !(a.kind === 'task' && !a.doneAt)));
	const KINDS = [
		{ value: 'task', label: 'Task' },
		{ value: 'call', label: 'Call' },
		{ value: 'meeting', label: 'Meeting' },
		{ value: 'note', label: 'Note' }
	];
	const ICONS = {
		task: CheckSquare,
		call: Phone,
		meeting: CalendarDays,
		note: StickyNote,
		email: Mail
	};
	const when = (v: string | null | undefined) => (v ? new Date(v).toLocaleString() : '');
	const overdue = (a: Activity) => !!a.dueAt && Date.parse(a.dueAt) < Date.now();

	const done =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: result.type === 'success' });
			const said = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (said as { message?: string } | undefined)?.message;
			if (result.type === 'success') {
				toast.push({ title: message ?? 'Done.', tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				toast.push({ title: message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const field =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg';
</script>

{#snippet item(a: Activity)}
	{@const Icon = ICONS[a.kind]}
	<li class="flex gap-3 py-2.5">
		<span class="mt-0.5 text-fg-subtle"><Icon size={15} aria-hidden="true" /></span>
		<div class="min-w-0 flex-1">
			<p class="flex flex-wrap items-center gap-2 text-sm">
				<span class="font-semibold text-fg">{a.subject}</span>
				{#if a.kind === 'email'}<Badge>{a.direction === 'in' ? 'From them' : 'To them'}</Badge>{/if}
				{#if a.outcome}<Badge>{a.outcome}</Badge>{/if}
				{#if a.kind === 'task' && !a.doneAt && overdue(a)}<Badge tone="danger">Overdue</Badge>{/if}
			</p>
			<p class="text-xs text-fg-subtle">
				{#if a.kind === 'task'}
					{a.ownerName ? `For ${a.ownerName}` : ''}{a.dueAt ? ` · due ${when(a.dueAt)}` : ''}
				{:else if a.kind === 'meeting'}
					{when(a.startsAt)}{a.location ? ` · ${a.location}` : ''}
				{:else}
					{a.createdByName ?? a.contactName ?? ''} · {when(a.createdAt)}
				{/if}
				{#if a.durationMinutes}· {a.durationMinutes} min{/if}
				{#if a.contactName && a.kind !== 'email'}· with {a.contactName}{/if}
				{#if a.kind === 'email' && a.contactName}· {a.contactName}{/if}
			</p>
			{#if a.body}
				<p class="mt-1 line-clamp-4 text-sm whitespace-pre-wrap text-fg-muted">{a.body}</p>
			{/if}
		</div>
		{#if canWrite && a.kind === 'task'}
			<form method="POST" action="?/completeActivity" use:enhance={done()}>
				<input type="hidden" name="activityId" value={a.id} />
				<input type="hidden" name="done" value={a.doneAt ? 'no' : 'yes'} />
				<Button type="submit" size="sm" variant="ghost">{a.doneAt ? 'Reopen' : 'Done'}</Button>
			</form>
		{/if}
	</li>
{/snippet}

<Card title="Activity">
	{#snippet actions()}
		{#if canWrite && !adding}
			<Button size="sm" onclick={() => (adding = true)}>Log or plan something</Button>
		{/if}
	{/snippet}
	<div class="flex flex-col gap-4">
		{#if adding}
			<form
				method="POST"
				action="?/addActivity"
				class="flex flex-col gap-3 rounded-control border border-border p-3"
				use:enhance={done(() => (adding = false))}
			>
				<div class="grid gap-3 sm:grid-cols-[9rem_minmax(0,1fr)]">
					<Field label="Kind" for="activity-kind">
						<Select id="activity-kind" name="kind" bind:value={kind} options={KINDS} />
					</Field>
					<Field label={kind === 'note' ? 'Note' : 'What'} for="activity-subject">
						<Input id="activity-subject" name="subject" required maxlength={300} />
					</Field>
				</div>
				{#if kind === 'task' || kind === 'meeting'}
					<div class="grid gap-3 sm:grid-cols-2">
						<Field label={kind === 'task' ? 'Due' : 'When'} for="activity-when">
							<Input id="activity-when" name="when" type="datetime-local" />
						</Field>
						{#if kind === 'task'}
							<Field label="For" for="activity-owner">
								<Select
									id="activity-owner"
									name="ownerId"
									options={[{ value: '', label: 'Me' }, ...people]}
								/>
							</Field>
						{:else}
							<label class="flex items-end gap-2 pb-2 text-sm text-fg">
								<input type="checkbox" name="held" /> It's happened
							</label>
						{/if}
					</div>
				{:else if kind === 'call'}
					<div class="grid gap-3 sm:grid-cols-2">
						<Field label="Minutes" for="activity-duration">
							<Input
								id="activity-duration"
								name="durationMinutes"
								type="number"
								min={0}
								max={1440}
							/>
						</Field>
						<Field label="How it went" for="activity-outcome">
							<Select
								id="activity-outcome"
								name="outcome"
								options={[
									{ value: 'connected', label: 'Connected' },
									{ value: 'voicemail', label: 'Left a voicemail' },
									{ value: 'no answer', label: 'No answer' }
								]}
							/>
						</Field>
					</div>
				{/if}
				<Field label="Details" for="activity-body">
					<textarea id="activity-body" name="body" rows="3" maxlength="20000" class={field}
					></textarea>
				</Field>
				<div class="flex gap-2">
					<Button type="submit" size="sm" variant="primary">Add</Button>
					<Button size="sm" variant="ghost" onclick={() => (adding = false)}>Cancel</Button>
				</div>
			</form>
		{/if}
		{#if open.length > 0}
			<div>
				<p class="text-xs font-bold text-fg-subtle">To do</p>
				<ul class="divide-y divide-border" aria-label="Open tasks">
					{#each open as a (a.id)}{@render item(a)}{/each}
				</ul>
			</div>
		{/if}
		{#if past.length > 0}
			<ul class="divide-y divide-border" aria-label="Activity history">
				{#each past as a (a.id)}{@render item(a)}{/each}
			</ul>
		{:else if open.length === 0 && !adding}
			<p class="text-sm text-fg-muted">
				Nothing yet. Log calls, meetings and notes, or plan the next step.
			</p>
		{/if}
	</div>
</Card>
