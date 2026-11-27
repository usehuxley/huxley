<script lang="ts">
	import { page } from '$app/state';
	import { enhance } from '$app/forms';
	import { invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { RefreshCw, Trash2 } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Select } from '@usehuxley/ui';
	import { PRIORITIES } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	// Keep the organization (`?org=`) when a form posts back.
	const act = (name: string) =>
		page.url.searchParams.get('org')
			? `?org=${encodeURIComponent(page.url.searchParams.get('org') ?? '')}&/${name}`
			: `?/${name}`;
	let latest = $derived(data.evaluations[0]);
	const pct = (s: { correct: number; of: number }) =>
		s.of === 0 ? '–' : `${Math.round((s.correct * 1000) / s.of) / 10}%`;
	const priority = (p: string | null | undefined) =>
		PRIORITIES.find((x) => x.value === p)?.short ?? '–';
</script>

<svelte:head><title>AI triage · Settings · Huxley</title></svelte:head>

<div class="flex flex-col gap-4">
	<p class="text-sm text-fg-muted">
		The built-in <strong>AI triage</strong> pack suggests a queue and priority for each ticket a client
		raises. Measure how often it's right on a labeled sample set before you rely on it.
	</p>

	{#if data.agreement}
		<Card
			title="On live tickets"
			description="Resolved tickets in the last 90 days that had a suggestion."
		>
			<dl class="grid gap-4 sm:grid-cols-3">
				<div>
					<dt class="text-sm text-fg-muted">Queue matched</dt>
					<dd class="text-2xl font-semibold text-fg tabular-nums">{pct(data.agreement.queue)}</dd>
					<dd class="text-xs text-fg-subtle">
						{data.agreement.queue.correct} of {data.agreement.queue.of}
					</dd>
				</div>
				<div>
					<dt class="text-sm text-fg-muted">Priority matched</dt>
					<dd class="text-2xl font-semibold text-fg tabular-nums">
						{pct(data.agreement.priority)}
					</dd>
					<dd class="text-xs text-fg-subtle">
						{data.agreement.priority.correct} of {data.agreement.priority.of}
					</dd>
				</div>
				<div>
					<dt class="text-sm text-fg-muted">Applied before anyone worked them</dt>
					<dd class="text-2xl font-semibold text-fg tabular-nums">{data.agreement.applied}</dd>
					<dd class="text-xs text-fg-subtle">of {data.agreement.tickets} tickets</dd>
				</div>
			</dl>
		</Card>
	{/if}

	<Card
		title="Accuracy evaluation"
		description="Runs the “Triage new tickets with AI” workflow on every sample and compares its answers with the labels."
	>
		<div class="flex flex-wrap items-center gap-2">
			<form method="POST" action={act('evaluate')} use:enhance>
				<Button type="submit" variant="primary" disabled={data.samples.length === 0}>
					Run evaluation
				</Button>
			</form>
			{#if latest?.pending}
				<Button variant="ghost" onclick={() => invalidateAll()}>
					<RefreshCw size={16} /> Refresh
				</Button>
			{/if}
			{#if form && 'evaluate' in form}<p class="text-sm text-fg-muted" role="status">
					{form.evaluate}
				</p>{/if}
		</div>
		{#if latest}
			<div class="mt-4 flex flex-col gap-3" aria-label="Latest evaluation" role="region">
				<div class="flex flex-wrap items-baseline gap-3">
					<span class="text-3xl font-semibold text-fg tabular-nums">
						{latest.accuracy === null || latest.accuracy === undefined
							? '–'
							: `${latest.accuracy}%`}
					</span>
					<span class="text-sm text-fg-muted">queues right (target {latest.target}%)</span>
					{#if latest.passed === true}
						<Badge tone="success">Meets the target</Badge>
					{:else if latest.passed === false}
						<Badge tone="danger">Below the target</Badge>
					{:else}
						<Badge tone="info">{latest.pending} of {latest.total} still running</Badge>
					{/if}
				</div>
				<p class="text-sm text-fg-muted">
					Priority right: {pct(latest.priority)} · {latest.answered} answered · {latest.failed} failed
					· started {new Date(latest.startedAt).toLocaleString()}
				</p>
				{#if latest.misses.length > 0}
					<div class="overflow-x-auto">
						<table class="w-full text-sm" aria-label="Misses">
							<thead class="text-left text-fg-muted">
								<tr>
									<th class="py-1 pr-3 font-medium">Text</th>
									<th class="py-1 pr-3 font-medium">Expected</th>
									<th class="py-1 pr-3 font-medium">Answered</th>
									<th class="py-1 font-medium">Why</th>
								</tr>
							</thead>
							<tbody>
								{#each latest.misses as m (m.position)}
									<tr class="border-t border-border align-top">
										<td class="py-1 pr-3 text-fg">{m.text}</td>
										<td class="py-1 pr-3 whitespace-nowrap text-fg">
											{m.expectedQueue ?? '–'} · {priority(m.expectedPriority)}
										</td>
										<td class="py-1 pr-3 whitespace-nowrap text-fg">
											{#if m.failed}
												{#if m.runId}
													<a
														href={resolve('/(app)/runs/[id]', { id: m.runId })}
														class="text-accent hover:underline"
													>
														The run failed
													</a>
												{:else}The run failed{/if}
											{:else}
												{m.queue ?? '(none)'} · {priority(m.priority)}
											{/if}
										</td>
										<td class="py-1 text-fg-muted">{m.reason ?? ''}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</div>
		{/if}
	</Card>

	<Card title="Labeled samples" description="Examples with the queue and priority they should get.">
		<div class="flex flex-col gap-4">
			<form method="POST" action={act('fromTickets')} use:enhance>
				<Button type="submit">Add recently resolved tickets</Button>
			</form>
			<form
				method="POST"
				action={act('add')}
				class="grid gap-3 sm:grid-cols-[1fr_12rem_9rem_auto] sm:items-end"
				use:enhance
			>
				<Field label="Ticket text" for="sample-text">
					<textarea
						id="sample-text"
						name="text"
						rows="2"
						maxlength="8000"
						class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
					></textarea>
				</Field>
				<Field label="Right queue" for="sample-queue">
					<Select
						id="sample-queue"
						name="queueId"
						options={[
							{ value: '', label: '(none)' },
							...data.queues.map((q) => ({ value: q.id, label: q.name }))
						]}
					/>
				</Field>
				<Field label="Right priority" for="sample-priority">
					<Select
						id="sample-priority"
						name="priority"
						options={[
							{ value: '', label: '–' },
							...PRIORITIES.map((p) => ({ value: p.value, label: p.short }))
						]}
					/>
				</Field>
				<Button type="submit">Add sample</Button>
			</form>
			{#if form && 'sample' in form}<p class="text-sm text-fg-muted" role="status">
					{form.sample}
				</p>{/if}
			{#if data.samples.length === 0}
				<p class="text-sm text-fg-muted">No samples yet.</p>
			{:else}
				<ul class="flex flex-col divide-y divide-border" aria-label="Samples">
					{#each data.samples as s (s.id)}
						<li class="flex items-start justify-between gap-3 py-2">
							<div class="min-w-0">
								<p class="line-clamp-2 text-sm text-fg">{s.text}</p>
								<p class="text-xs text-fg-muted">
									{s.queueName ?? '(no queue)'} · {priority(s.priority)}
									{#if s.ticketId}
										· <a
											href={resolve('/(app)/tickets/[id]', { id: s.ticketId })}
											class="text-accent hover:underline"
										>
											from a ticket
										</a>
									{/if}
								</p>
							</div>
							<form method="POST" action={act('remove')} use:enhance>
								<input type="hidden" name="id" value={s.id} />
								<Button type="submit" variant="ghost" aria-label="Remove sample">
									<Trash2 size={16} />
								</Button>
							</form>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	</Card>
</div>
