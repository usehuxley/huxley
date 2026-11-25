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
		CalendarDays,
		ChartColumn,
		CircleCheck,
		CircleUserRound,
		Clock,
		ClipboardList,
		Database,
		FileCode,
		House,
		ListTodo,
		Plug,
		Settings,
		Sparkles,
		Ticket,
		UsersRound,
		Workflow
	} from '@lucide/svelte';
	import { AppShell, Button, CommandPalette, type Command, type NavItem } from '@usehuxley/ui';
	import OrgSwitcher from '#lib/orgs/OrgSwitcher.svelte';
	import { visibleSections } from '#lib/permissions.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// Modules arrive phase by phase (see docs/IMPLEMENTATION_PLAN.md).
	let canSeeSettings = $derived(
		visibleSections(data.user.grantedPermissions, data.user.activeModules).length > 0
	);
	let nav: NavItem[] = $derived([
		{ href: '/', label: 'Home', icon: House },
		...(data.user.activeModules.includes('servicedesk') &&
		data.user.grantedPermissions.includes('ticket:read')
			? [
					{ href: '/my-work', label: 'My work', icon: ListTodo },
					{ href: '/time', label: 'My time', icon: Clock },
					{ href: '/dispatch', label: 'Dispatch', icon: CalendarDays }
				]
			: []),
		...(data.user.grantedPermissions.includes('workflow:read')
			? [{ href: '/workflows', label: 'Workflows', icon: Workflow }]
			: []),
		...(data.user.grantedPermissions.includes('script:read')
			? [{ href: '/scripts', label: 'Scripts', icon: FileCode }]
			: []),
		...(data.user.grantedPermissions.includes('integration:read')
			? [{ href: '/integrations', label: 'Integrations', icon: Plug }]
			: []),
		...(data.user.grantedPermissions.includes('dataset:read')
			? [{ href: '/datasets', label: 'Datasets', icon: Database }]
			: []),
		...(data.user.grantedPermissions.includes('ai:use')
			? [{ href: '/assistant', label: 'Assistant', icon: Sparkles }]
			: []),
		{ href: '/approvals', label: 'Approvals', icon: CircleCheck },
		...(data.user.grantedPermissions.some((p) => p === 'form:submit' || p === 'form:manage')
			? [{ href: '/forms', label: 'Forms', icon: ClipboardList }]
			: []),
		...(data.user.activeModules.includes('servicedesk') &&
		data.user.grantedPermissions.includes('ticket:read')
			? [{ href: '/tickets', label: 'Tickets', icon: Ticket }]
			: [{ href: '/tickets', label: 'Tickets', icon: Ticket, soon: true }]),
		...(data.user.activeModules.includes('crm') && data.user.grantedPermissions.includes('crm:read')
			? [{ href: '/clients', label: 'Clients', icon: Building2 }]
			: [{ href: '/clients', label: 'Clients', icon: Building2, soon: true }]),
		...(data.user.activeModules.includes('workforce') &&
		data.user.grantedPermissions.includes('workforce:read')
			? [{ href: '/people', label: 'People', icon: UsersRound }]
			: []),
		...(data.user.activeModules.includes('cmdb') &&
		data.user.grantedPermissions.includes('asset:read')
			? [{ href: '/assets', label: 'Assets', icon: Boxes }]
			: [{ href: '/assets', label: 'Assets', icon: Boxes, soon: true }]),
		...(data.user.activeModules.includes('knowledge') &&
		data.user.grantedPermissions.includes('kb:read')
			? [{ href: '/knowledge', label: 'Knowledge', icon: BookOpen }]
			: [{ href: '/knowledge', label: 'Knowledge', icon: BookOpen, soon: true }]),
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
			id: 'integrations',
			label: 'Go to Integrations',
			keywords: ['openapi', 'api', 'connectors'],
			group: 'Navigate',
			run: () => goto(resolve('/(app)/integrations'))
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
	let paletteQuery = $state('');
	let found: Command[] = $state([]);
	let searching = $state(false);

	type Hit = { kind: string; id: string; title: string; subtitle: string };
	const KIND_LABELS: Record<string, string> = {
		ticket: 'Ticket',
		client: 'Client',
		contact: 'Contact',
		workflow: 'Workflow',
		person: 'Person'
	};

	/** Where a search hit opens. */
	function hitPath(hit: Hit): string {
		switch (hit.kind) {
			case 'ticket':
				return resolve('/(app)/tickets/[id]', { id: hit.id });
			case 'client':
				return resolve('/(app)/clients/[id]', { id: hit.id });
			case 'contact':
				return resolve('/(app)/contacts/[id]', { id: hit.id });
			case 'workflow':
				return resolve('/(app)/workflows/[id]', { id: hit.id });
			default:
				return data.user.activeModules.includes('workforce') &&
					data.user.grantedPermissions.includes('workforce:read')
					? resolve('/(app)/people/[id]', { id: hit.id })
					: resolve('/(app)/settings/users/[id]', { id: hit.id });
		}
	}

	// Searches as you type, a moment after you stop.
	$effect(() => {
		const q = paletteQuery.trim();
		if (q.length < 2) {
			found = [];
			searching = false;
			return;
		}
		searching = true;
		const controller = new AbortController();
		const timer = setTimeout(() => {
			fetch(`/search?q=${encodeURIComponent(q)}`, { signal: controller.signal })
				.then((res) => (res.ok ? (res.json() as Promise<Hit[]>) : []))
				.then((hits) => {
					found = hits.map((hit) => ({
						id: `${hit.kind}:${hit.id}`,
						label: hit.title,
						detail: hit.subtitle,
						group: KIND_LABELS[hit.kind] ?? hit.kind,
						// eslint-disable-next-line svelte/no-navigation-without-resolve -- hitPath resolves
						run: () => void goto(hitPath(hit))
					}));
					searching = false;
				})
				.catch(() => {
					searching = false;
				});
		}, 200);
		return () => {
			clearTimeout(timer);
			controller.abort();
		};
	});

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
		<!-- On phones: the profile, which has Sign out, instead of the name. -->
		<a
			href={resolve('/(app)/profile')}
			class="shrink-0 rounded-control p-2 text-fg-muted hover:bg-surface-sunken hover:text-fg sm:hidden"
			aria-label={`Account: ${data.user.displayName}`}
		>
			<CircleUserRound size={18} />
		</a>
		<form method="POST" action="/logout" class="hidden sm:block">
			<Button type="submit" variant="ghost" size="sm">Sign out</Button>
		</form>
	{/snippet}
	{@render children()}
</AppShell>

<CommandPalette
	{commands}
	bind:open={paletteOpen}
	bind:query={paletteQuery}
	{found}
	{searching}
	placeholder="Search tickets, clients, people… or run a command"
/>
