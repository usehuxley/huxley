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

	let { children }: LayoutProps = $props();

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
			id: 'login',
			label: 'Go to sign-in page',
			group: 'Navigate',
			run: () => goto(resolve('/(auth)/login'))
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
		<Button href="/login" variant="ghost" size="sm">Sign in</Button>
	{/snippet}
	{@render children()}
</AppShell>

<CommandPalette {commands} bind:open={paletteOpen} />
