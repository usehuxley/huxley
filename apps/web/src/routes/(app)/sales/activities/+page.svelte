<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, EmptyState, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	type Activity = (typeof data.open)[number];
	// Midnight tonight (local), as a timestamp.
	const now = new Date(Date.now());
	const tomorrow = Date.parse(now.toDateString()) + 86_400_000;
	let overdue = $derived(data.open.filter((a) => a.dueAt && Date.parse(a.dueAt) < Date.now()));
	let today = $derived(
		data.open.filter(
			(a) =>
				!overdue.includes(a) &&
				(a.dueAt ?? a.startsAt) &&
				Date.parse((a.dueAt ?? a.startsAt)!) < tomorrow
		)
	);
	let later = $derived(data.open.filter((a) => !overdue.includes(a) && !today.includes(a)));
	const when = (v: string | null | undefined) => (v ? new Date(v).toLocaleString() : 'No date');
	const about = (a: Activity) =>
		a.opportunityId
			? {
					href: resolve('/(app)/sales/opportunities/[id]', { id: a.opportunityId }),
					name: a.opportunityName
				}
			: a.leadId
				? { href: resolve('/(app)/sales/leads/[id]', { id: a.leadId }), name: a.leadName }
				: null;
</script>

{#snippet list(items: Activity[], label: string)}
	<ul class="divide-y divide-border" aria-label={label}>
		{#each items as a (a.id)}
			{@const on = about(a)}
			<li class="flex items-center justify-between gap-3 py-2.5">
				<div class="min-w-0">
					<p class="flex flex-wrap items-center gap-2 text-sm">
						<span class="font-semibold text-fg">{a.subject}</span>
						<Badge>{a.kind === 'meeting' ? 'Meeting' : a.kind === 'task' ? 'Task' : a.kind}</Badge>
					</p>
					<p class="text-xs text-fg-subtle">
						{when(a.dueAt ?? a.startsAt)}{#if on}
							· <a href={on.href} class="text-accent hover:underline">{on.name}</a>{/if}
					</p>
				</div>
				{#if data.canWrite && a.kind === 'task' && !a.doneAt}
					<form
						method="POST"
						action="?/completeActivity"
						use:enhance={() =>
							async ({ result, update }) => {
								await update();
								if (result.type === 'success') toast.push({ title: 'Done.', tone: 'success' });
							}}
					>
						<input type="hidden" name="activityId" value={a.id} />
						<input type="hidden" name="done" value="yes" />
						<Button type="submit" size="sm">Done</Button>
					</form>
				{/if}
			</li>
		{/each}
	</ul>
{/snippet}

<svelte:head>
	<title>My activities · Sales · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-5">
	<div>
		<h1 class="text-[26px] leading-tight font-extrabold tracking-tight text-fg">My activities</h1>
		<p class="mt-1 text-sm text-fg-subtle">Your sales tasks and meetings, soonest first.</p>
	</div>
	{#if data.open.length === 0}
		<EmptyState
			title="Nothing to do"
			description="Tasks and meetings you plan on leads and opportunities show here."
		/>
	{:else}
		{#if overdue.length}<Card title="Overdue">{@render list(overdue, 'Overdue')}</Card>{/if}
		{#if today.length}<Card title="Today">{@render list(today, 'Today')}</Card>{/if}
		{#if later.length}<Card title="Coming up">{@render list(later, 'Coming up')}</Card>{/if}
	{/if}
	{#if data.recent.length}
		<Card title="Done lately">{@render list(data.recent, 'Done lately')}</Card>
	{/if}
</div>
