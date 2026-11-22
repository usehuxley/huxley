<script lang="ts" module>
	export type ThemePreference = 'system' | 'light' | 'dark';
	const STORAGE_KEY = 'hx-theme';
</script>

<script lang="ts">
	import { Monitor, Moon, Sun } from '@lucide/svelte';

	let preference = $state<ThemePreference>('system');

	function apply(next: ThemePreference) {
		const root = document.documentElement;
		if (next === 'system') root.removeAttribute('data-theme');
		else root.setAttribute('data-theme', next);
	}

	$effect(() => {
		// Restore the saved choice once on mount. Storage can throw in private
		// mode or when blocked, so treat it as optional.
		try {
			const saved = localStorage.getItem(STORAGE_KEY);
			if (saved === 'light' || saved === 'dark') preference = saved;
		} catch {
			/* fall back to system */
		}
	});

	$effect(() => {
		apply(preference);
		try {
			if (preference === 'system') localStorage.removeItem(STORAGE_KEY);
			else localStorage.setItem(STORAGE_KEY, preference);
		} catch {
			/* not persisted */
		}
	});

	const order: ThemePreference[] = ['system', 'light', 'dark'];
	const labels: Record<ThemePreference, string> = {
		system: 'Theme: system',
		light: 'Theme: light',
		dark: 'Theme: dark'
	};

	function cycle() {
		preference = order[(order.indexOf(preference) + 1) % order.length];
	}
</script>

<button
	type="button"
	class="rounded-control p-2 text-fg-muted hover:bg-surface-sunken hover:text-fg"
	aria-label={labels[preference]}
	title={labels[preference]}
	onclick={cycle}
>
	{#if preference === 'light'}
		<Sun size={18} />
	{:else if preference === 'dark'}
		<Moon size={18} />
	{:else}
		<Monitor size={18} />
	{/if}
</button>
