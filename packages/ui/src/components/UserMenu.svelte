<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Popover } from 'bits-ui';
	import { LogOut, Monitor, Moon, Sun } from '@lucide/svelte';
	import type { ThemePreference } from './ThemeToggle.svelte';

	/**
	 * The signed-in person's menu: who they are, their links (e.g. the profile),
	 * the theme, and signing out. Opens from their initials.
	 */
	type Props = {
		name: string;
		email?: string;
		/** Where signing out posts. */
		signOutAction?: string;
		/** Links and other entries, between the header and the theme. */
		items?: Snippet<[close: () => void]>;
		/** Which side the menu opens on: beside the rail, or below the top bar. */
		side?: 'right' | 'bottom';
		/** Classes for the button with the initials. */
		class?: string;
	};

	let {
		name,
		email,
		signOutAction = '/logout',
		items,
		side = 'right',
		class: className = ''
	}: Props = $props();

	const STORAGE_KEY = 'hx-theme';
	let open = $state(false);
	let theme = $state<ThemePreference>('system');

	let initials = $derived(
		name
			.split(/\s+/)
			.filter(Boolean)
			.slice(0, 2)
			.map((w) => w.charAt(0).toUpperCase())
			.join('') || '?'
	);

	$effect(() => {
		try {
			const saved = localStorage.getItem(STORAGE_KEY);
			if (saved === 'light' || saved === 'dark') theme = saved;
		} catch {
			/* system */
		}
	});

	function choose(next: ThemePreference) {
		theme = next;
		const root = document.documentElement;
		if (next === 'system') root.removeAttribute('data-theme');
		else root.setAttribute('data-theme', next);
		try {
			if (next === 'system') localStorage.removeItem(STORAGE_KEY);
			else localStorage.setItem(STORAGE_KEY, next);
		} catch {
			/* not kept */
		}
	}

	const THEMES = [
		{ value: 'system', label: 'System', icon: Monitor },
		{ value: 'light', label: 'Light', icon: Sun },
		{ value: 'dark', label: 'Dark', icon: Moon }
	] as const;
</script>

<Popover.Root bind:open>
	<Popover.Trigger
		class="grid size-9 place-items-center rounded-full bg-[#2f6db3] text-xs font-bold text-white hover:ring-2 hover:ring-eye/60 {className}"
		aria-label="Your account: {name}"
		title={name}
	>
		{initials}
	</Popover.Trigger>
	<Popover.Portal>
		<Popover.Content
			{side}
			align="end"
			sideOffset={10}
			class="z-50 w-64 rounded-card border border-border bg-surface-raised p-1.5 text-sm text-fg shadow-overlay"
		>
			<div class="border-b border-border px-2.5 pt-1.5 pb-2.5">
				<p class="truncate font-bold">{name}</p>
				{#if email}<p class="truncate text-xs text-fg-subtle">{email}</p>{/if}
			</div>
			{#if items}
				<div class="flex flex-col py-1">{@render items(() => (open = false))}</div>
			{/if}
			<div class="border-t border-border px-2.5 py-2.5">
				<p class="mb-1.5 text-xs font-semibold text-fg-subtle" id="theme-label">Theme</p>
				<div
					class="grid grid-cols-3 gap-1 rounded-control bg-surface-sunken p-0.5"
					role="radiogroup"
					aria-labelledby="theme-label"
				>
					{#each THEMES as t (t.value)}
						<button
							type="button"
							role="radio"
							aria-checked={theme === t.value}
							class="flex items-center justify-center gap-1.5 rounded-[5px] px-2 py-1 text-xs font-semibold {theme ===
							t.value
								? 'bg-surface text-fg shadow-card'
								: 'text-fg-muted hover:text-fg'}"
							onclick={() => choose(t.value)}
						>
							<t.icon size={13} />
							{t.label}
						</button>
					{/each}
				</div>
			</div>
			<form method="POST" action={signOutAction} class="border-t border-border pt-1">
				<button
					type="submit"
					class="flex w-full items-center gap-2 rounded-control px-2.5 py-2 text-left font-medium text-fg-muted hover:bg-surface-sunken hover:text-fg"
				>
					<LogOut size={15} /> Sign out
				</button>
			</form>
		</Popover.Content>
	</Popover.Portal>
</Popover.Root>
