<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import {
		Bell,
		Briefcase,
		Building2,
		CircleUserRound,
		House,
		Inbox,
		Settings,
		Workflow
	} from '@lucide/svelte';
	import {
		Button,
		CommandPalette,
		ThemeToggle,
		Workbench,
		type Command,
		type Space,
		type SpaceMenuItem
	} from '@usehuxley/ui';
	import OrgSwitcher from '#lib/orgs/OrgSwitcher.svelte';
	import { visibleSections } from '#lib/permissions.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	let can = (p: string) => data.user.grantedPermissions.includes(p);
	let on = (m: string) => data.user.activeModules.includes(m);
	let canSeeSettings = $derived(
		visibleSections(data.user.grantedPermissions, data.user.activeModules).length > 0
	);

	function item(show: boolean, href: string, label: string): SpaceMenuItem[] {
		return show ? [{ href, label }] : [];
	}

	// Five spaces, each with its own menu. Modules arrive phase by phase
	// (docs/IMPLEMENTATION_PLAN.md) and appear only when on and permitted.
	let spaces: Space[] = $derived.by(() => {
		const desk = on('servicedesk') && can('ticket:read');
		const all: Space[] = [
			{
				id: 'home',
				label: 'Home',
				icon: House,
				href: '/',
				paths: ['/', '/my-work', '/time', '/approvals', '/notifications', '/assistant', '/profile'],
				menu: [
					{
						items: [
							{ href: '/', label: 'Home' },
							...item(desk, '/my-work', 'My work'),
							...item(desk, '/time', 'My time'),
							{ href: '/approvals', label: 'Approvals' },
							{ href: '/notifications', label: 'Notifications' },
							...item(can('ai:use'), '/assistant', 'Assistant')
						]
					}
				]
			},
			{
				id: 'desk',
				label: 'Desk',
				icon: Inbox,
				href: '/tickets',
				paths: ['/tickets', '/dispatch', '/changes'],
				menu: [
					{
						items: [
							...item(desk, '/tickets', 'Tickets'),
							...item(desk, '/dispatch', 'Dispatch'),
							...item(on('change') && can('ticket:read'), '/changes', 'Changes')
						]
					}
				]
			},
			{
				id: 'clients',
				label: 'Clients',
				icon: Building2,
				href: '/clients',
				paths: ['/clients', '/contacts', '/assets', '/knowledge'],
				menu: [
					{
						items: [
							...item(on('crm') && can('crm:read'), '/clients', 'Clients'),
							...item(on('cmdb') && can('asset:read'), '/assets', 'Assets'),
							...item(on('knowledge') && can('kb:read'), '/knowledge', 'Knowledge')
						]
					}
				]
			},
			{
				id: 'engine',
				label: 'Engine',
				icon: Workflow,
				href: '/workflows',
				paths: [
					'/workflows',
					'/runs',
					'/forms',
					'/scripts',
					'/integrations',
					'/datasets',
					'/vault'
				],
				menu: [
					{
						items: [
							...item(can('workflow:read'), '/workflows', 'Workflows'),
							...item(can('form:submit') || can('form:manage'), '/forms', 'Forms'),
							...item(can('script:read'), '/scripts', 'Scripts'),
							...item(can('integration:read'), '/integrations', 'Integrations'),
							...item(can('dataset:read'), '/datasets', 'Datasets')
						]
					}
				]
			},
			{
				id: 'business',
				label: 'Business',
				icon: Briefcase,
				href: '/projects',
				paths: ['/projects', '/catalog', '/dashboards', '/people', '/timesheets'],
				menu: [
					{
						items: [
							...item(on('projects') && can('project:read'), '/projects', 'Projects'),
							...item(on('contracts') && can('catalog:read'), '/catalog', 'Catalog'),
							...item(can('dashboard:read'), '/dashboards', 'Dashboards'),
							...item(on('workforce') && can('workforce:read'), '/people', 'People'),
							...item(
								on('workforce') && (can('timesheet:approve') || can('workforce:manage')),
								'/timesheets',
								'Timesheets'
							)
						]
					}
				]
			}
		];
		// A space with nothing in it is left off; the rest open on their first page.
		return all
			.filter((s) => s.id === 'home' || (s.menu?.[0]?.items.length ?? 0) > 0)
			.map((s) => ({ ...s, href: s.menu?.[0]?.items[0]?.href ?? s.href }));
	});

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

<Workbench
	brand="Huxley"
	{spaces}
	currentPath={page.url.pathname}
	currentSearch={page.url.search}
	onSearch={() => (paletteOpen = true)}
>
	{#snippet railEnd()}
		{#if canSeeSettings}
			<a
				href={resolve('/(app)/settings')}
				class="grid size-10 place-items-center rounded-lg text-rail-fg hover:bg-rail-raised hover:text-rail-fg-active {page.url.pathname.startsWith(
					'/settings'
				)
					? 'bg-rail-raised text-rail-fg-active'
					: ''}"
				aria-label="Settings"
				aria-current={page.url.pathname.startsWith('/settings') ? 'page' : undefined}
			>
				<Settings size={19} />
			</a>
		{/if}
	{/snippet}
	{#snippet topbar()}
		<OrgSwitcher current={data.currentOrg} />
		<span class="hidden sm:contents"><ThemeToggle /></span>
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
</Workbench>

<CommandPalette
	{commands}
	bind:open={paletteOpen}
	bind:query={paletteQuery}
	{found}
	{searching}
	placeholder="Search tickets, clients, people… or run a command"
/>
