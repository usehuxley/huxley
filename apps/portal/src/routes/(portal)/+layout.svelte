<script lang="ts">
	import { page } from '$app/state';
	import { BookOpen, Boxes, ClipboardList, House, Ticket } from '@lucide/svelte';
	import { AppShell, Button, type NavItem } from '@usehuxley/ui';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	// The portal is white-labeled per MSP in Phase 7. Until then it uses Huxley defaults.
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
		<Button href="/login" variant="ghost" size="sm">Sign in</Button>
	{/snippet}
	{@render children()}
</AppShell>
