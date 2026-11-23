<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Building2, ChevronsUpDown } from '@lucide/svelte';
	import { CommandPalette, toast, type Command } from '@usehuxley/ui';
	import type { OrgOption } from '#lib/orgContext.ts';

	let { current }: { current: { id: string; name: string } | null } = $props();

	let open = $state(false);
	let orgs = $state<OrgOption[] | null>(null);

	async function show() {
		open = true;
		if (orgs === null) {
			const response = await fetch('/org-context').catch(() => null);
			orgs = response?.ok ? ((await response.json()) as OrgOption[]) : [];
		}
	}

	async function choose(org: OrgOption | null) {
		const response = await fetch('/org-context', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ orgId: org?.id ?? null })
		}).catch(() => null);
		if (!response?.ok) {
			toast.push({ title: 'Could not switch organization.', tone: 'danger' });
			return;
		}
		await invalidateAll();
		if (org) await goto(resolve('/(app)/settings/organizations/[id]', { id: org.id }));
	}

	let commands: Command[] = $derived([
		...(current
			? [
					{
						id: 'all',
						label: 'All organizations',
						group: 'Clear',
						run: () => void choose(null)
					}
				]
			: []),
		...(orgs ?? []).map((o) => ({
			id: o.id,
			label: o.parentName ? `${o.name} · ${o.parentName}` : o.name,
			keywords: [o.kind],
			group: o.kind === 'provider' ? 'Provider' : 'Client',
			run: () => void choose(o)
		}))
	]);
</script>

<button
	type="button"
	onclick={show}
	class="flex max-w-56 items-center gap-1.5 rounded-control px-2 py-1.5 text-sm font-medium text-fg-muted hover:bg-surface-sunken hover:text-fg"
	aria-label="Switch organization, current: {current?.name ?? 'all organizations'}"
	data-testid="org-switcher"
>
	<Building2 size={16} class="shrink-0" />
	<span class="truncate">{current?.name ?? 'All organizations'}</span>
	<ChevronsUpDown size={14} class="shrink-0 text-fg-subtle" />
</button>

<CommandPalette
	{commands}
	bind:open
	hotkey={false}
	title="Switch organization"
	placeholder={orgs === null ? 'Loading organizations…' : 'Find an organization…'}
/>
