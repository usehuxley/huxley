<script lang="ts">
	import { resolve } from '$app/paths';
	import { Zap } from '@lucide/svelte';
	import { Card } from '@usehuxley/ui';
	import type { ActionForm } from './actions.server.ts';

	/** Actions to run on a record: links to fill them in with it as context. */
	let {
		actions,
		orgId,
		contactId,
		ticketId
	}: { actions: ActionForm[]; orgId: string; contactId?: string; ticketId?: string } = $props();

	const query = $derived(
		[
			`org=${encodeURIComponent(orgId)}`,
			contactId ? `contact=${encodeURIComponent(contactId)}` : '',
			ticketId ? `ticket=${encodeURIComponent(ticketId)}` : ''
		]
			.filter(Boolean)
			.join('&')
	);
</script>

{#if actions.length > 0}
	<Card title="Actions">
		<ul class="flex flex-col gap-1" aria-label="Actions">
			{#each actions as action (action.id)}
				<li>
					<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
					<a
						href={`${resolve('/(app)/forms/[id]', { id: action.id })}?${query}`}
						class="flex items-start gap-2 rounded-control px-2 py-1.5 text-sm text-fg hover:bg-surface-sunken"
					>
						<Zap size={14} class="mt-0.5 shrink-0 text-accent" />
						<span class="min-w-0">
							<span class="block">{action.name}</span>
							{#if action.description}
								<span class="block truncate text-xs text-fg-muted">{action.description}</span>
							{/if}
						</span>
					</a>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
				</li>
			{/each}
		</ul>
	</Card>
{/if}
