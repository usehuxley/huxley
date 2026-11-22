<script lang="ts">
	import { ShieldCheck } from '@lucide/svelte';
	import { Badge, Button, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const timeFormat = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'medium'
	});

	function tone(action: string) {
		if (action.includes('failed') || action.includes('locked')) return 'danger' as const;
		if (action.startsWith('auth.')) return 'info' as const;
		return 'neutral' as const;
	}

	function actor(e: (typeof data.page.items)[number]) {
		if (e.actorName) return e.actorName;
		if (e.actorType === 'system') return 'Huxley';
		if (e.actorType === 'anonymous') return 'Anonymous';
		return 'Deleted user';
	}

	function summary(details: Record<string, unknown>): string {
		const parts: string[] = [];
		for (const [k, v] of Object.entries(details)) {
			if (v === null || typeof v === 'object') continue;
			parts.push(`${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`);
		}
		return parts.join(' · ');
	}
</script>

<svelte:head>
	<title>Audit log · Settings · Huxley</title>
</svelte:head>

<section class="rounded-card border border-border bg-surface">
	<header class="flex flex-wrap items-center gap-3 border-b border-border px-5 py-3">
		<ShieldCheck size={18} class="text-fg-muted" />
		<h2 class="text-sm font-semibold text-fg">Audit log</h2>
		<p class="text-sm text-fg-muted">Append-only and tamper-evident.</p>
		<form method="GET" class="ml-auto flex gap-2">
			<label for="audit-action" class="sr-only">Filter by action prefix</label>
			<Input
				id="audit-action"
				name="action"
				value={data.action}
				placeholder="Filter, e.g. auth."
				class="w-48"
			/>
			<Button type="submit" size="md">Filter</Button>
		</form>
	</header>

	<div class="overflow-x-auto">
		<table class="w-full text-left text-sm">
			<thead
				class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
			>
				<tr>
					<th scope="col" class="px-4 py-2.5 font-medium">When</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Who</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Action</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Where</th>
					<th scope="col" class="px-4 py-2.5 font-medium">Details</th>
				</tr>
			</thead>
			<tbody>
				{#each data.page.items as e (e.id)}
					<tr class="border-b border-border align-top last:border-0">
						<td class="px-4 py-2.5 whitespace-nowrap text-fg-muted">
							{timeFormat.format(new Date(e.occurredAt))}
						</td>
						<td class="px-4 py-2.5">
							<span class="text-fg">{actor(e)}</span>
							<span class="block text-xs text-fg-subtle">{e.via}{e.ip ? ` · ${e.ip}` : ''}</span>
						</td>
						<td class="px-4 py-2.5"><Badge tone={tone(e.action)}>{e.action}</Badge></td>
						<td class="px-4 py-2.5 text-fg-muted">{e.orgName ?? '—'}</td>
						<td
							class="max-w-md truncate px-4 py-2.5 text-fg-muted"
							title={JSON.stringify(e.details)}
						>
							{summary(e.details)}
						</td>
					</tr>
				{:else}
					<tr>
						<td colspan="5" class="px-4 py-10 text-center text-fg-muted">No matching events.</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>

	{#if data.page.nextCursor}
		<footer class="border-t border-border px-5 py-3 text-right">
			<Button
				size="sm"
				href="?cursor={encodeURIComponent(data.page.nextCursor)}{data.action
					? `&action=${encodeURIComponent(data.action)}`
					: ''}"
			>
				Older events
			</Button>
		</footer>
	{/if}
</section>
