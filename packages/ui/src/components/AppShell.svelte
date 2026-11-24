<script lang="ts" module>
	import type { Component } from 'svelte';

	export type NavItem = {
		href: string;
		label: string;
		icon?: Component<{ size?: number | string; class?: string }>;
		/** Marks items for modules that don't exist yet. */
		soon?: boolean;
	};
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Menu, Search, X } from '@lucide/svelte';
	import ThemeToggle from './ThemeToggle.svelte';

	type Props = {
		/** Product or tenant name shown at the top of the sidebar. */
		brand: string;
		nav: NavItem[];
		/** Current pathname, used to highlight the active nav item. */
		currentPath: string;
		/** Opens the command palette. */
		onSearch?: () => void;
		/** Right side of the top bar, e.g. the user menu. */
		topbar?: Snippet;
		children: Snippet;
	};

	let { brand, nav, currentPath, onSearch, topbar, children }: Props = $props();

	let mobileOpen = $state(false);

	function isActive(href: string) {
		return href === '/' ? currentPath === '/' : currentPath.startsWith(href);
	}
</script>

<div class="flex min-h-screen bg-bg">
	<!-- Sidebar: fixed on desktop, slide-over on mobile. -->
	<aside
		class="fixed inset-y-0 left-0 z-40 flex w-64 flex-col border-r border-border bg-surface transition-transform md:translate-x-0 {mobileOpen
			? 'translate-x-0'
			: '-translate-x-full'}"
	>
		<div class="flex h-14 items-center justify-between px-4">
			<a href="/" class="flex items-center gap-2 font-semibold text-fg">
				<span
					class="flex size-7 items-center justify-center rounded-control bg-accent text-sm font-bold text-accent-fg"
					aria-hidden="true"
				>
					H
				</span>
				{brand}
			</a>
			<button
				type="button"
				class="rounded-control p-1.5 text-fg-muted hover:bg-surface-sunken md:hidden"
				aria-label="Close navigation"
				onclick={() => (mobileOpen = false)}
			>
				<X size={18} />
			</button>
		</div>
		<nav class="flex-1 overflow-y-auto px-2 py-2" aria-label="Main navigation">
			<ul class="flex flex-col gap-0.5">
				{#each nav as item (item.href)}
					{@const active = isActive(item.href)}
					<li>
						<a
							href={item.href}
							aria-current={active ? 'page' : undefined}
							onclick={() => (mobileOpen = false)}
							class="flex items-center gap-3 rounded-control px-3 py-2 text-sm font-medium {active
								? 'bg-accent-soft text-fg'
								: 'text-fg-muted hover:bg-surface-sunken hover:text-fg'}"
						>
							{#if item.icon}<item.icon size={18} />{/if}
							<span class="flex-1">{item.label}</span>
							{#if item.soon}<span class="text-xs text-fg-subtle">Soon</span>{/if}
						</a>
					</li>
				{/each}
			</ul>
		</nav>
	</aside>

	{#if mobileOpen}
		<button
			type="button"
			class="fixed inset-0 z-30 bg-overlay md:hidden"
			aria-label="Close navigation"
			onclick={() => (mobileOpen = false)}
		></button>
	{/if}

	<div class="flex min-w-0 flex-1 flex-col md:pl-64">
		<header
			class="sticky top-0 z-20 flex h-14 items-center gap-3 border-b border-border bg-surface/90 px-4 backdrop-blur"
		>
			<button
				type="button"
				class="shrink-0 rounded-control p-1.5 text-fg-muted hover:bg-surface-sunken md:hidden"
				aria-label="Open navigation"
				onclick={() => (mobileOpen = true)}
			>
				<Menu size={20} />
			</button>
			{#if onSearch}
				<button
					type="button"
					onclick={onSearch}
					aria-label="Search or jump to…"
					class="flex h-9 w-9 shrink-0 items-center justify-center gap-2 rounded-control border border-border bg-surface-sunken text-sm text-fg-subtle hover:border-border-strong sm:w-full sm:max-w-md sm:shrink sm:justify-start sm:px-3"
				>
					<Search size={16} class="shrink-0" />
					<span class="hidden flex-1 text-left sm:inline">Search or jump to…</span>
					<kbd class="hidden rounded border border-border px-1.5 font-mono text-xs sm:inline">
						⌘K
					</kbd>
				</button>
			{/if}
			<div class="ml-auto flex min-w-0 items-center gap-1">
				<ThemeToggle />
				{#if topbar}{@render topbar()}{/if}
			</div>
		</header>
		<main class="flex-1 px-4 py-6 md:px-8">{@render children()}</main>
	</div>
</div>
