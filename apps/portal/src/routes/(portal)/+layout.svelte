<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		BookOpen,
		Boxes,
		ChartColumn,
		CircleCheck,
		ClipboardList,
		House,
		Ticket,
		UsersRound
	} from '@lucide/svelte';
	import { AppShell, Button, type NavItem } from '@usehuxley/ui';
	import { accentStyle, DEFAULT_BRANDING } from '#lib/branding.ts';
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();
	let branding = $derived(data.me.branding ?? DEFAULT_BRANDING);

	// Parts of the portal follow what's turned on for their company.
	let nav: NavItem[] = $derived(
		[
			{ href: '/', label: 'Home', icon: House, module: null },
			{ href: '/tickets', label: 'Tickets', icon: Ticket, module: 'servicedesk' },
			{ href: '/requests', label: 'Request something', icon: ClipboardList, module: null },
			{ href: '/assets', label: 'Devices', icon: Boxes, module: 'cmdb' },
			{ href: '/knowledge', label: 'Help articles', icon: BookOpen, module: 'knowledge' },
			{ href: '/approvals', label: 'Approvals', icon: CircleCheck, module: 'approvals' },
			{ href: '/people', label: 'People', icon: UsersRound, module: 'people' },
			{ href: '/dashboards', label: 'Reports', icon: ChartColumn, module: 'dashboards' }
		]
			.filter((item) =>
				item.module === 'approvals'
					? data.me.permissions.includes('portal.approval:decide')
					: item.module === 'people'
						? data.me.permissions.includes('portal.contacts:manage')
						: item.module === 'dashboards'
							? data.me.permissions.includes('portal.dashboard:read')
							: item.module === null || data.me.modules.includes(item.module)
			)
			.map((item) => ({ href: item.href, label: item.label, icon: item.icon }))
	);
</script>

<div style={accentStyle(branding.accentColor)}>
	<AppShell brand={branding.name} logo={branding.logo} {nav} currentPath={page.url.pathname}>
		{#snippet topbar()}
			<a href={resolve('/(portal)/profile')} class="text-sm text-fg-muted hover:text-fg">
				{data.me.name} · {data.me.orgName}
			</a>
			<form method="POST" action="/logout">
				<Button type="submit" variant="ghost" size="sm">Sign out</Button>
			</form>
		{/snippet}
		{@render children()}
	</AppShell>
</div>
