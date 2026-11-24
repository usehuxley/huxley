<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { ATTENTION, nextDue } from './attention.ts';
	import { ago, categoryTone, priorityOf } from './labels.ts';

	type Ticket = components['schemas']['Ticket'];

	let {
		tickets,
		label,
		attention = {},
		showAssignee = false
	}: {
		tickets: Ticket[];
		label: string;
		attention?: Record<string, string>;
		showAssignee?: boolean;
	} = $props();
</script>

<ul class="flex flex-col divide-y divide-border" aria-label={label}>
	{#each tickets as ticket (ticket.id)}
		{@const priority = priorityOf(ticket.priority)}
		{@const why = attention[ticket.id] ? ATTENTION[attention[ticket.id]] : undefined}
		{@const due = nextDue(ticket)}
		<li
			class="flex flex-col gap-1.5 py-2.5 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between sm:gap-4"
		>
			<div class="min-w-0 sm:flex-1">
				<a
					class="block truncate text-sm font-medium text-fg hover:text-accent"
					href={resolve('/(app)/tickets/[id]', { id: ticket.id })}
				>
					{ticket.title}
				</a>
				<span class="text-xs text-fg-muted tabular-nums">
					{ticket.reference} · {ticket.orgName}{ticket.queueName
						? ` · ${ticket.queueName}`
						: ''}{showAssignee && ticket.assigneeTeamName ? ` · ${ticket.assigneeTeamName}` : ''}
				</span>
			</div>
			<span class="flex flex-wrap items-center gap-1.5">
				<!-- New shows as the status already. -->
				{#if why && attention[ticket.id] !== 'new'}<Badge tone={why.tone}>{why.label}</Badge>{/if}
				<Badge tone={categoryTone(ticket.statusCategory)}>{ticket.statusName}</Badge>
				<Badge tone={priority.tone}>{priority.short}</Badge>
				{#if due && !ticket.slaPaused}
					<time class="text-xs whitespace-nowrap text-fg-muted" datetime={due}>
						due {ago(due)}
					</time>
				{/if}
			</span>
		</li>
	{/each}
</ul>
