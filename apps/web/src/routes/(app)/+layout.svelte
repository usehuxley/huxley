<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		BookOpen,
		Boxes,
		Building2,
		ChartColumn,
		House,
		Settings,
		Ticket,
		Workflow
	} from '@lucide/svelte';
	import { AppShell, Button, CommandPalette, type Command, type NavItem } from '@usehuxley/ui';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// Modules arrive phase by phase (see docs/IMPLEMENTATION_PLAN.md).
	const nav: NavItem[] = [
		{ href: '/', label: 'Home', icon: House },
		{ href: '/workflows', label: 'Workflows', icon: Workflow, soon: true },
		{ href: '/tickets', label: 'Tickets', icon: Ticket, soon: true },
		{ href: '/clients', label: 'Clients', icon: Building2, soon: true },
		{ href: '/assets', label: 'Assets', icon: Boxes, soon: true },
		{ href: '/knowledge', label: 'Knowledge', icon: BookOpen, soon: true },
		{ href: '/reports', label: 'Reports', icon: ChartColumn, soon: true },
		{ href: '/settings', label: 'Settings', icon: Settings, soon: true }
	];

	const commands: Command[] = [
		{ id: 'home', label: 'Go to Home', group: 'Navigate', run: () => goto(resolve('/(app)')) },
		{
			id: 'profile',
			label: 'Your profile and sessions',
			keywords: ['password', 'account', 'sessions'],
			group: 'Navigate',
			run: () => goto(resolve('/(app)/profile'))
		}
	];

	let paletteOpen = $state(false);
</script>

<AppShell
	brand="Huxley"
	{nav}
	currentPath={page.url.pathname}
	onSearch={() => (paletteOpen = true)}
>
	{#snippet topbar()}
		<a
			href={resolve('/(app)/profile')}
			class="hidden rounded-control px-2 py-1.5 text-sm font-medium text-fg-muted hover:bg-surface-sunken hover:text-fg sm:block"
		>
			{data.user.displayName}
		</a>
		<form method="POST" action="/logout">
			<Button type="submit" variant="ghost" size="sm">Sign out</Button>
		</form>
	{/snippet}
	{@render children()}
</AppShell>

<CommandPalette {commands} bind:open={paletteOpen} />
