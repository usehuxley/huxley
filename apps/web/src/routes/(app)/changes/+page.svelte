<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	const day = (t: string) =>
		new Date(t).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
	const time = (t: string) =>
		new Date(t).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	const riskTone = (r: string | null | undefined) =>
		r === 'high' ? 'danger' : r === 'medium' ? 'warning' : 'neutral';
	let byDay = $derived(
		Object.entries(
			data.calendar.changes.reduce<Record<string, typeof data.calendar.changes>>((acc, c) => {
				const key = new Date(c.plannedStart).toDateString();
				(acc[key] ??= []).push(c);
				return acc;
			}, {})
		)
	);
</script>

<svelte:head><title>Changes · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Changes</h1>
		<p class="mt-1 text-fg-muted">
			Changes planned in the next four weeks, and the freeze windows.
		</p>
	</div>

	{#if data.calendar.freezes.length > 0 || data.canFreeze}
		<Card
			title="Freeze windows"
			description="No normal changes in these windows (emergencies can still go ahead)."
		>
			{#if data.calendar.freezes.length > 0}
				<ul class="flex flex-col gap-1 text-sm" aria-label="Freezes">
					{#each data.calendar.freezes as f (f.id)}
						<li class="flex items-center justify-between gap-2">
							<span class="text-fg">
								<strong>{f.name}</strong> · {day(f.startsAt)}
								{time(f.startsAt)} to {day(f.endsAt)}
								{time(f.endsAt)}
								<span class="text-fg-muted">({f.orgName} and below)</span>
							</span>
							{#if data.canFreeze}
								<form method="POST" action="?/unfreeze" use:enhance>
									<input type="hidden" name="id" value={f.id} />
									<Button type="submit" variant="ghost" aria-label="Remove {f.name}">
										<Trash2 size={16} />
									</Button>
								</form>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if data.canFreeze}
				<form
					method="POST"
					action="?/freeze"
					class="mt-3 grid gap-3 sm:grid-cols-4 sm:items-end"
					use:enhance
				>
					<Field label="Name" for="fz-name"><Input id="fz-name" name="name" required /></Field>
					<Field label="From" for="fz-start">
						<Input id="fz-start" name="startsAt" type="datetime-local" required />
					</Field>
					<Field label="To" for="fz-end">
						<Input id="fz-end" name="endsAt" type="datetime-local" required />
					</Field>
					<Field label="For" for="fz-org">
						<Select id="fz-org" name="orgId" options={data.orgs} />
					</Field>
					<div><Button type="submit">Add freeze</Button></div>
				</form>
				{#if form && 'freeze' in form}<p class="mt-2 text-sm text-fg-muted" role="status">
						{form.freeze}
					</p>{/if}
			{/if}
		</Card>
	{/if}

	<Card title="Planned changes">
		{#if byDay.length === 0}
			<p class="text-sm text-fg-muted">No changes planned in the next four weeks.</p>
		{:else}
			<div class="flex flex-col gap-4" aria-label="Change calendar" role="list">
				{#each byDay as [key, changes] (key)}
					<section role="listitem">
						<h2 class="mb-1 text-sm font-semibold text-fg">{day(changes[0].plannedStart)}</h2>
						<ul class="flex flex-col gap-1">
							{#each changes as c (c.ticketId)}
								<li class="flex flex-wrap items-center gap-2 text-sm">
									<span class="w-28 shrink-0 text-fg-muted tabular-nums">
										{time(c.plannedStart)}–{time(c.plannedEnd)}
									</span>
									<a
										href={resolve('/(app)/tickets/[id]', { id: c.ticketId })}
										class="text-accent hover:underline"
									>
										{c.reference} · {c.title}
									</a>
									<span class="text-fg-muted">{c.orgName}</span>
									<Badge tone={riskTone(c.riskLevel)}>{c.riskLevel ?? 'unassessed'} risk</Badge>
									<Badge>{c.state}</Badge>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
			</div>
		{/if}
	</Card>
</div>
