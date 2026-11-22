<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Blocks } from '@lucide/svelte';
	import { Badge, Button, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let byKey = $derived(new Map(data.modules.map((m) => [m.key, m])));
	let enabledCount = $derived(data.modules.filter((m) => m.enabled).length);

	/**
	 * Dependencies that are currently off and would be turned on with this
	 * module, including indirect ones, dependencies first.
	 */
	function missingDeps(requires: string[]): string[] {
		const order: string[] = [];
		const visit = (key: string) => {
			if (order.includes(key)) return;
			for (const dep of byKey.get(key)?.requires ?? []) visit(dep);
			order.push(key);
		};
		for (const key of requires) visit(key);
		return order.filter((k) => !byKey.get(k)?.enabled).map((k) => byKey.get(k)?.name ?? k);
	}

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update();
			if (result.type === 'success') {
				const changed = (result.data?.changed as string[] | undefined) ?? [];
				const names = changed.map((k) => byKey.get(k)?.name ?? k).join(', ');
				toast.push({
					title: result.data?.enabled ? `Turned on: ${names}` : `Turned off: ${names}`,
					tone: 'success'
				});
			} else if (result.type === 'failure') {
				toast.push({
					title: String(result.data?.message ?? 'Could not change it'),
					tone: 'danger'
				});
			}
		};
	};
</script>

<svelte:head>
	<title>Modules · Settings · Huxley</title>
</svelte:head>

<section class="rounded-card border border-border bg-surface">
	<header class="flex flex-wrap items-center gap-2 border-b border-border px-5 py-3">
		<Blocks size={18} class="text-fg-muted" />
		<h2 class="text-sm font-semibold text-fg">Feature modules</h2>
		<p class="text-sm text-fg-muted">
			The automation engine is always on. With no modules, Huxley runs as an automation engine only.
			Turning a module off keeps its data.
		</p>
	</header>

	<ul class="divide-y divide-border">
		{#each data.modules as m (m.key)}
			{@const missing = missingDeps(m.requires)}
			<li class="flex flex-wrap items-center gap-4 px-5 py-4">
				<div class="min-w-0 flex-1">
					<p class="flex flex-wrap items-center gap-2 font-medium text-fg">
						{m.name}
						{#if m.active}
							<Badge tone="success">Active</Badge>
						{:else if m.enabled}
							<Badge tone="info">On: arrives in Phase {m.phase}</Badge>
						{:else if !m.released}
							<Badge>Coming in Phase {m.phase}</Badge>
						{/if}
					</p>
					<p class="mt-0.5 text-sm text-fg-muted">{m.description}</p>
					{#if m.requires.length > 0}
						<p class="mt-0.5 text-xs text-fg-subtle">
							Requires {m.requires.map((k) => byKey.get(k)?.name ?? k).join(', ')}{!m.enabled &&
							missing.length > 0
								? `. Turning it on also turns on ${missing.join(', ')}.`
								: ''}
						</p>
					{/if}
				</div>
				<form method="POST" action="?/toggle" use:enhance={notify}>
					<input type="hidden" name="key" value={m.key} />
					<input type="hidden" name="enabled" value={String(!m.enabled)} />
					<Button type="submit" size="sm" variant={m.enabled ? 'secondary' : 'primary'}>
						{m.enabled ? 'Turn off' : 'Turn on'}
					</Button>
				</form>
			</li>
		{/each}
	</ul>

	<footer class="border-t border-border px-5 py-3 text-sm text-fg-muted">
		{enabledCount === 0
			? 'No modules on: automation engine only.'
			: `${enabledCount} of ${data.modules.length} modules on.`}
	</footer>
</section>
