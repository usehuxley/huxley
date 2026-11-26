<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { BookOpen, Boxes, ClipboardList, House, Ticket } from '@lucide/svelte';
	import { AppShell, Button, type NavItem } from '@usehuxley/ui';
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();

	const nav: NavItem[] = [
		{ href: '/', label: 'Home', icon: House },
		{ href: '/tickets', label: 'Tickets', icon: Ticket, soon: true },
		{ href: '/requests', label: 'Request something', icon: ClipboardList, soon: true },
		{ href: '/assets', label: 'Devices', icon: Boxes, soon: true },
		{ href: '/knowledge', label: 'Help articles', icon: BookOpen, soon: true }
	];
</script>

<AppShell brand="Support portal" {nav} currentPath={page.url.pathname}>
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
