<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import {
		Bell,
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
	import OrgSwitcher from '#lib/orgs/OrgSwitcher.svelte';
	import { visibleSections } from '#lib/permissions.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// Modules arrive phase by phase (see docs/IMPLEMENTATION_PLAN.md).
	let canSeeSettings = $derived(visibleSections(data.user.grantedPermissions).length > 0);
	let nav: NavItem[] = $derived([
		{ href: '/', label: 'Home', icon: House },
		...(data.user.grantedPermissions.includes('workflow:read')
			? [{ href: '/workflows', label: 'Workflows', icon: Workflow }]
			: []),
		{ href: '/tickets', label: 'Tickets', icon: Ticket, soon: true },
		{ href: '/clients', label: 'Clients', icon: Building2, soon: true },
		{ href: '/assets', label: 'Assets', icon: Boxes, soon: true },
		{ href: '/knowledge', label: 'Knowledge', icon: BookOpen, soon: true },
		{ href: '/reports', label: 'Reports', icon: ChartColumn, soon: true },
		...(canSeeSettings ? [{ href: '/settings', label: 'Settings', icon: Settings }] : [])
	]);

	const commands: Command[] = [
		{ id: 'home', label: 'Go to Home', group: 'Navigate', run: () => goto(resolve('/(app)')) },
		{
			id: 'settings',
			label: 'Settings: organizations, users, audit log',
			keywords: ['admin', 'roles', 'permissions', 'organizations', 'users', 'audit'],
			group: 'Navigate',
			run: () => goto(resolve('/(app)/settings'))
		},
		{
			id: 'workflows',
			label: 'Go to Workflows',
			keywords: ['automation', 'runs'],
			group: 'Navigate',
			run: () => goto(resolve('/(app)/workflows'))
		},
		{
			id: 'profile',
			label: 'Your profile and sessions',
			keywords: ['password', 'account', 'sessions'],
			group: 'Navigate',
			run: () => goto(resolve('/(app)/profile'))
		}
	];

	let paletteOpen = $state(false);

	// Live unread count: the server pushes it over Server-Sent Events.
	let liveUnread = $state<number | null>(null);
	let unread = $derived(liveUnread ?? data.unreadNotifications);
	onMount(() => {
		const source = new EventSource('/notifications/stream');
		source.addEventListener('unread', (e) => {
			liveUnread = Number((e as MessageEvent<string>).data) || 0;
		});
		return () => source.close();
	});
</script>

<AppShell
	brand="Huxley"
	{nav}
	currentPath={page.url.pathname}
	onSearch={() => (paletteOpen = true)}
>
	{#snippet topbar()}
		<OrgSwitcher current={data.currentOrg} />
		<a
			href={resolve('/(app)/notifications')}
			class="relative rounded-control p-2 text-fg-muted hover:bg-surface-sunken hover:text-fg"
			aria-label={unread > 0 ? `Notifications, ${unread} unread` : 'Notifications'}
		>
			<Bell size={18} />
			{#if unread > 0}
				<span
					class="absolute -top-0.5 -right-0.5 min-w-4 rounded-full bg-danger px-1 text-center text-[10px] leading-4 font-semibold text-white"
					data-testid="unread-count"
				>
					{unread > 99 ? '99+' : unread}
				</span>
			{/if}
		</a>
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
