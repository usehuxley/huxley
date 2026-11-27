<script lang="ts">
	import { resolve } from '$app/paths';
	import { Zap } from '@lucide/svelte';
	import { Card } from '@usehuxley/ui';
	import ActionDrawer from './ActionDrawer.svelte';
	import type { ActionForm } from './actions.server.ts';

	/** Actions to run on a record: each opens beside the page, with the record as context. */
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

	let running = $state<ActionForm | null>(null);
	let open = $state(false);
</script>

{#if actions.length > 0}
	<Card title="Actions">
		<ul class="flex flex-col gap-1.5" aria-label="Actions">
			{#each actions as action (action.id)}
				<li>
					<button
						type="button"
						class="group flex w-full items-center gap-2.5 rounded-control border border-border bg-surface px-2.5 py-2 text-left text-sm text-fg hover:border-accent hover:bg-accent-soft"
						onclick={() => {
							running = action;
							open = true;
						}}
					>
						<span
							class="grid size-7 flex-none place-items-center rounded-md bg-surface-sunken text-fg-muted group-hover:bg-surface group-hover:text-accent"
						>
							<Zap size={14} />
						</span>
						<span class="min-w-0 flex-1">
							<span class="block font-semibold">{action.name}</span>
							{#if action.description}
								<span class="block truncate text-xs text-fg-subtle">{action.description}</span>
							{/if}
						</span>
						<span class="text-xs font-bold text-accent" aria-hidden="true">Run</span>
					</button>
				</li>
			{/each}
		</ul>
	</Card>
{/if}

{#if running}
	<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string -->
	<ActionDrawer
		bind:open
		action={running}
		href={`${resolve('/(app)/forms/[id]', { id: running.id })}?${query}`}
	/>
{/if}
