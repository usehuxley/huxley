<script lang="ts" module>
	export type BadgeTone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';

	let { tone = 'neutral', children }: { tone?: BadgeTone; children: Snippet } = $props();

	const tones: Record<BadgeTone, string> = {
		neutral: 'bg-surface-sunken text-fg-muted',
		accent: 'bg-accent-soft text-accent',
		success: 'bg-success-soft text-success',
		warning: 'bg-warning-soft text-warning',
		danger: 'bg-danger-soft text-danger',
		info: 'bg-info-soft text-info'
	};
</script>

<span
	class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium {tones[tone]}"
>
	{@render children()}
</span>
