<script lang="ts" module>
	export type { Space, SpaceMenuGroup, SpaceMenuItem } from '../workbench';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Menu, Search, X } from '@lucide/svelte';
	import defaultMark from '../assets/huxley-mark.png';
	import { clockTone, clockUsed, tray, type TrayItem } from '../tray.svelte';
	import type { Space } from '../workbench';

	type Props = {
		/** Product or tenant name, used as the mark's label. */
		brand: string;
		/** The mark at the top of the rail; defaults to the Huxley butler. */
		mark?: string | null;
		spaces: Space[];
		/** Current pathname. */
		currentPath: string;
		/** Current query string (with `?`), to highlight menu items that filter a page. */
		currentSearch?: string;
		/** Opens the command palette. */
		onSearch?: () => void;
		/** Right side of the top bar. */
		topbar?: Snippet;
		/** Bottom of the rail, e.g. settings and the user. */
		railEnd?: Snippet;
		children: Snippet;
	};

	let {
		brand,
		mark = null,
		spaces,
		currentPath,
		currentSearch = '',
		onSearch,
		topbar,
		railEnd,
		children
	}: Props = $props();

	let menuOpen = $state(false);
	let now = $state(Date.now());

	$effect(() => {
		tray.restore();
		const timer = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(timer);
	});

	function within(path: string, prefix: string) {
		if (prefix === '/') return path === '/';
		return path === prefix || path.startsWith(prefix + '/');
	}

	let activeSpace = $derived.by(() => {
		let best: Space | undefined;
		let bestLen = -1;
		for (const s of spaces) {
			for (const p of s.paths) {
				if (within(currentPath, p) && p.length > bestLen) {
					best = s;
					bestLen = p.length;
				}
			}
		}
		return best;
	});

	let activeItem = $derived.by(() => {
		const items = (activeSpace?.menu ?? []).flatMap((g) => g.items);
		const exact = items.find((i) => i.href === currentPath + currentSearch);
		if (exact) return exact.href;
		let best: string | undefined;
		let bestLen = -1;
		for (const i of items) {
			if (i.href.includes('?')) continue;
			if (within(currentPath, i.href) && i.href.length > bestLen) {
				best = i.href;
				bestLen = i.href.length;
			}
		}
		return best;
	});

	let hasMenu = $derived((activeSpace?.menu ?? []).some((g) => g.items.length > 0));

	function trayActive(item: TrayItem) {
		return item.href.split('?')[0] === currentPath;
	}

	function ringStyle(item: TrayItem) {
		if (!item.clock) return '';
		const used = Math.min(1, clockUsed(item.clock, now));
		const tone = clockTone(clockUsed(item.clock, now));
		const color = item.clock.paused
			? 'var(--hx-fg-subtle)'
			: tone === 'danger'
				? 'var(--hx-danger)'
				: tone === 'warn'
					? 'var(--hx-warning)'
					: 'var(--hx-success)';
		return `--used:${used};--ring:${color}`;
	}

	function clockText(item: TrayItem) {
		if (!item.clock) return '';
		if (item.clock.paused) return 'SLA paused';
		const left = new Date(item.clock.due).getTime() - now;
		const mins = Math.round(Math.abs(left) / 60_000);
		const text =
			mins >= 1440
				? `${Math.round(mins / 1440)}d`
				: `${Math.floor(mins / 60)}:${String(mins % 60).padStart(2, '0')}`;
		return left < 0 ? `SLA breached ${text} ago` : `SLA ${text} left`;
	}

	function close(event: MouseEvent, key: string) {
		event.preventDefault();
		event.stopPropagation();
		tray.close(key);
	}
</script>

{#snippet spaceMenu()}
	<h2 class="px-2.5 pt-0.5 pb-3 text-lg font-extrabold tracking-tight text-fg">
		{activeSpace?.label}
	</h2>
	{#each activeSpace?.menu ?? [] as group, gi (gi)}
		{#if group.items.length > 0}
			{#if group.label}
				<p
					class="px-2.5 pt-4 pb-1.5 text-[11.5px] font-bold tracking-[0.08em] text-fg-subtle uppercase"
				>
					{group.label}
				</p>
			{/if}
			<ul class="flex flex-col gap-0.5">
				{#each group.items as item (item.href)}
					{@const on = item.href === activeItem}
					<li>
						<a
							href={item.href}
							aria-current={on ? 'page' : undefined}
							onclick={() => (menuOpen = false)}
							class="flex items-center gap-2 rounded-control px-2.5 py-1.5 text-sm {on
								? 'bg-accent-soft font-semibold text-fg'
								: 'font-medium text-fg-muted hover:bg-surface-sunken hover:text-fg'}"
						>
							<span class="min-w-0 flex-1 truncate">{item.label}</span>
							{#if item.count != null}
								<span
									class="font-mono text-xs tabular-nums {item.hot && item.count > 0
										? 'font-semibold text-danger'
										: 'text-fg-subtle'}"
								>
									{item.count}
								</span>
							{/if}
						</a>
					</li>
				{/each}
			</ul>
		{/if}
	{/each}
{/snippet}

<div class="workbench flex min-h-dvh bg-bg text-fg">
	<!-- Rail: the five spaces. A bottom bar on phones. -->
	<nav
		class="fixed inset-x-0 bottom-0 z-30 flex items-center justify-around bg-rail px-2 pt-1.5 pb-[calc(0.375rem+env(safe-area-inset-bottom,0px))] md:sticky md:top-0 md:h-dvh md:w-[76px] md:flex-none md:flex-col md:justify-start md:gap-1 md:py-3"
		aria-label="Spaces"
	>
		<a
			href="/"
			class="mb-3 hidden size-11 place-items-center overflow-hidden rounded-full bg-[#e7ebef] md:grid"
			aria-label="{brand} home"
		>
			<img src={mark ?? defaultMark} alt="" class="size-10 object-contain" />
		</a>
		{#each spaces as space (space.id)}
			{@const on = space.id === activeSpace?.id}
			<a
				href={space.href}
				aria-current={on ? 'page' : undefined}
				class="relative flex flex-1 flex-col items-center gap-1 rounded-lg px-1 pt-2 pb-1.5 text-[11.5px] font-semibold md:w-[62px] md:flex-none {on
					? 'bg-rail-raised text-rail-fg-active'
					: 'text-rail-fg hover:bg-rail-raised hover:text-rail-fg-active'}"
			>
				<space.icon size={19} class={on ? 'text-eye' : ''} />
				<span>{space.label}</span>
				{#if space.badge}
					<span
						class="absolute top-1 right-2 grid h-[17px] min-w-[17px] place-items-center rounded-full bg-danger px-1 text-[10.5px] font-bold text-white"
						aria-label="{space.badge} need attention"
					>
						{space.badge > 99 ? '99+' : space.badge}
					</span>
				{/if}
			</a>
		{/each}
		<div class="hidden flex-1 md:block"></div>
		{#if railEnd}
			<div class="hidden flex-col items-center gap-2 md:flex">{@render railEnd()}</div>
		{/if}
	</nav>

	<!-- The open space's menu: a column on wide screens, a sheet below that. -->
	{#if hasMenu}
		<aside
			class="sticky top-0 hidden h-dvh w-56 flex-none overflow-y-auto border-r border-border bg-surface px-2.5 py-4 xl:block"
			aria-label="{activeSpace?.label} menu"
		>
			{@render spaceMenu()}
		</aside>
	{/if}

	<div class="flex min-w-0 flex-1 flex-col pb-[calc(4rem+env(safe-area-inset-bottom,0px))] md:pb-0">
		<header
			class="sticky top-0 z-20 flex min-h-14 items-stretch gap-3 border-b border-border bg-surface px-3 md:px-4"
		>
			{#if hasMenu}
				<button
					type="button"
					class="grid size-9 flex-none place-items-center self-center rounded-control text-fg-muted hover:bg-surface-sunken hover:text-fg xl:hidden"
					aria-label="Open {activeSpace?.label} menu"
					onclick={() => (menuOpen = true)}
				>
					<Menu size={19} />
				</button>
			{/if}
			{#if onSearch}
				<button
					type="button"
					onclick={onSearch}
					aria-label="Search or jump to…"
					class="flex h-9 w-9 flex-none items-center justify-center gap-2 self-center rounded-control border border-border bg-bg text-sm text-fg-subtle hover:border-border-strong sm:w-56 sm:justify-start sm:px-2.5"
				>
					<Search size={16} class="shrink-0" />
					<span class="hidden flex-1 text-left sm:inline">Search or jump…</span>
					<kbd
						class="hidden rounded border border-border bg-surface px-1.5 font-mono text-[11px] sm:inline"
					>
						⌘K
					</kbd>
				</button>
			{/if}

			<!-- The work tray: everything open, as tabs. -->
			<div
				class="tray flex min-w-0 flex-1 items-end gap-1 overflow-x-auto pt-2.5 [scrollbar-width:none]"
				role="tablist"
				aria-label="Open work"
			>
				{#if tray.items.length > 0}
					<span
						class="hidden flex-none self-center pr-1 text-[11.5px] font-bold tracking-[0.08em] text-fg-subtle uppercase lg:inline"
						aria-hidden="true"
					>
						Open
					</span>
				{/if}
				{#each tray.items as item, i (item.key)}
					{@const on = trayActive(item)}
					<!-- Tabs, not links: each is an open document, like a browser tab. -->
					<div
						role="presentation"
						class="tray-tab relative flex h-[42px] max-w-60 flex-none items-center gap-2 rounded-t-lg border border-b-0 pr-1 pl-2.5 {on
							? 'border-border-strong bg-bg text-fg'
							: 'border-border bg-bg text-fg-muted hover:text-fg'}"
						data-on={on || undefined}
					>
						<a
							href={item.href}
							class="flex min-w-0 items-center gap-2 after:absolute after:inset-0"
							role="tab"
							id="tray-tab-{i}"
							aria-selected={on}
							title={item.clock ? `${item.title} · ${clockText(item)}` : item.title}
						>
							{#if item.clock}
								<span class="ring" style={ringStyle(item)} aria-hidden="true"></span>
							{:else if item.kind === 'run'}
								<span
									class="grid size-[22px] flex-none place-items-center rounded-full bg-accent-soft text-accent {item.state ===
									'running'
										? 'animate-pulse'
										: ''}"
									aria-hidden="true"
								>
									<span class="size-2 rounded-full bg-current"></span>
								</span>
							{:else}
								<span
									class="grid size-[22px] flex-none place-items-center rounded-full bg-surface-sunken text-[11px] font-extrabold text-fg-muted"
									style={item.color ? `background:${item.color};color:#fff` : ''}
									aria-hidden="true"
								>
									{item.title.trim().charAt(0).toUpperCase()}
								</span>
							{/if}
							<span class="flex min-w-0 flex-col leading-tight">
								<span class="truncate text-[13px] font-semibold">{item.title}</span>
								<span class="truncate text-[11.5px] text-fg-subtle">
									{item.clock ? clockText(item) : (item.subtitle ?? '')}
								</span>
							</span>
						</a>
						<button
							type="button"
							class="relative z-10 grid size-[22px] flex-none place-items-center rounded text-fg-subtle hover:bg-surface-sunken hover:text-fg"
							aria-label="Close tab"
							aria-describedby="tray-tab-{i}"
							onclick={(e) => close(e, item.key)}
						>
							<X size={14} />
						</button>
					</div>
				{/each}
			</div>

			<div class="flex flex-none items-center gap-1">
				{#if topbar}{@render topbar()}{/if}
			</div>
		</header>
		<main class="flex-1 px-4 py-6 md:px-8">{@render children()}</main>
	</div>
</div>

{#if menuOpen && hasMenu}
	<button
		type="button"
		class="fixed inset-0 z-40 bg-overlay xl:hidden"
		aria-label="Close menu"
		onclick={() => (menuOpen = false)}
	></button>
	<aside
		class="fixed inset-y-0 left-0 z-50 w-72 overflow-y-auto bg-surface px-2.5 py-4 shadow-overlay xl:hidden"
		aria-label="{activeSpace?.label} menu"
	>
		<button
			type="button"
			class="absolute top-3 right-3 grid size-8 place-items-center rounded-control text-fg-muted hover:bg-surface-sunken"
			aria-label="Close menu"
			onclick={() => (menuOpen = false)}
		>
			<X size={18} />
		</button>
		{@render spaceMenu()}
	</aside>
{/if}

<style>
	.ring {
		width: 22px;
		height: 22px;
		flex: none;
		border-radius: 9999px;
		display: grid;
		place-items: center;
		background: conic-gradient(var(--ring) calc(var(--used) * 360deg), var(--hx-surface-sunken) 0);
	}
	.ring::after {
		content: '';
		width: 14px;
		height: 14px;
		border-radius: 9999px;
		background: var(--hx-bg);
	}
	/* Fade the last tab out when more are scrolled off to the right. */
	.tray {
		mask-image: linear-gradient(to right, #000 calc(100% - 24px), transparent);
	}
	.tray-tab[data-on]::after {
		content: '';
		position: absolute;
		left: 0;
		right: 0;
		bottom: -1px;
		height: 2px;
		background: var(--hx-bg);
	}
</style>
